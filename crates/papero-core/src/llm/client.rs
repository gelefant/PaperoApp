use crate::config::AppConfig;
use crate::credentials::SecretApiKey;
use crate::db::repos::docs::ContentSearchDocResult;
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::llm::citations::parse_citations;
use crate::llm::guard::{
    detect_refusal, has_cloud_consent, prepare_grounded_prompt, PreparedPrompt, REFUSAL_EN,
    REFUSAL_IT,
};
use crate::llm::provider::{
    LlmAnswerResult, LlmKeyVerifyResult, LlmModelInfo, LlmProvider, LlmUsage,
};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

fn build_http_client(is_local_override: bool) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(concat!("Papero/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(90));
    if is_local_override {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|e| CoreError::Network(format!("LLM HTTP client init failed: {e}")))
}

fn resolve_base_url(
    provider: LlmProvider,
    config_ollama_url: Option<&str>,
    base_url_override: Option<&str>,
) -> String {
    if let Some(ov) = base_url_override.map(str::trim).filter(|s| !s.is_empty()) {
        return ov.trim_end_matches('/').to_string();
    }
    if provider == LlmProvider::Ollama {
        if let Some(u) = config_ollama_url.map(str::trim).filter(|s| !s.is_empty()) {
            return u.trim_end_matches('/').to_string();
        }
    }
    provider.default_base_url().to_string()
}

fn require_cloud_key(
    provider: LlmProvider,
    api_key: Option<&SecretApiKey>,
) -> Result<&SecretApiKey> {
    match api_key {
        Some(k) if !k.is_empty() => Ok(k),
        _ => Err(CoreError::Config(format!(
            "Missing API key for {} in OS credential store",
            provider.display_name()
        ))),
    }
}

fn sanitize_error_snippet(raw: &str, api_key: Option<&SecretApiKey>) -> String {
    let mut cleaned = raw.trim().to_string();
    if let Some(k) = api_key {
        let secret = k.expose().trim();
        if !secret.is_empty() {
            cleaned = cleaned.replace(secret, "[REDACTED]");
        }
    }
    let truncated: String = cleaned.chars().take(280).collect();
    if cleaned.chars().count() > 280 {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Maps provider HTTP errors into clear, actionable messages while masking any secrets.
pub fn map_provider_http_error(
    provider: LlmProvider,
    status: StatusCode,
    body: &str,
    retry_after: Option<&str>,
    api_key: Option<&SecretApiKey>,
) -> CoreError {
    let detail = sanitize_error_snippet(body, api_key);
    let low = body.to_ascii_lowercase();
    let msg = match status.as_u16() {
        401 => format!(
            "{}: chiave API non valida o scaduta (HTTP 401). Verifica la chiave nelle Impostazioni. [{detail}]",
            provider.display_name()
        ),
        402 => format!(
            "{}: credito esaurito o fatturazione richiesta (HTTP 402). Controlla il tuo account provider. [{detail}]",
            provider.display_name()
        ),
        403 => {
            if low.contains("quota") || low.contains("billing") || low.contains("credit") {
                format!(
                    "{}: accesso negato / credito o quota insufficiente (HTTP 403). [{detail}]",
                    provider.display_name()
                )
            } else {
                format!(
                    "{}: chiave API priva dei permessi necessari (HTTP 403). [{detail}]",
                    provider.display_name()
                )
            }
        }
        429 => {
            let wait_hint = retry_after
                .map(|v| format!(" Riprova tra {v}s."))
                .unwrap_or_default();
            if provider == LlmProvider::Gemini {
                format!(
                    "Google Gemini API: limite di richieste o quota gratuita raggiunto (HTTP 429).{wait_hint} Attendi oppure riduci max passaggi/token. [{detail}]"
                )
            } else if low.contains("insufficient_quota") || low.contains("credit") {
                format!(
                    "{}: credito esaurito sul tuo account API (HTTP 429). [{detail}]",
                    provider.display_name()
                )
            } else {
                format!(
                    "{}: limite di velocità API raggiunto (HTTP 429).{wait_hint} [{detail}]",
                    provider.display_name()
                )
            }
        }
        404 => format!(
            "{}: modello o endpoint non trovato (HTTP 404). Aggiorna la lista modelli. [{detail}]",
            provider.display_name()
        ),
        code => format!(
            "{}: errore HTTP {code} dalla API. [{detail}]",
            provider.display_name()
        ),
    };
    CoreError::Network(msg)
}

/// Fetches the live model list from the configured provider's `/api/tags` or `/v1/models`
/// or `/v1beta/models` endpoint (zero hardcoded model catalogs).
pub async fn list_models(
    provider: LlmProvider,
    api_key: Option<&SecretApiKey>,
    ollama_url: Option<&str>,
    base_url_override: Option<&str>,
) -> Result<Vec<LlmModelInfo>> {
    if provider == LlmProvider::Disabled {
        return Ok(Vec::new());
    }

    let base = resolve_base_url(provider, ollama_url, base_url_override);
    let is_local = base.starts_with("http://127.0.0.1") || base.starts_with("http://localhost");
    let client = build_http_client(is_local)?;

    let req = match provider {
        LlmProvider::Disabled => unreachable!(),
        LlmProvider::Ollama => client.get(format!("{base}/api/tags")),
        LlmProvider::Anthropic => {
            let key = require_cloud_key(provider, api_key)?;
            client
                .get(format!("{base}/v1/models"))
                .header("x-api-key", key.expose())
                .header("anthropic-version", "2023-06-01")
        }
        LlmProvider::OpenAi => {
            let key = require_cloud_key(provider, api_key)?;
            client
                .get(format!("{base}/v1/models"))
                .bearer_auth(key.expose())
        }
        LlmProvider::Gemini => {
            let key = require_cloud_key(provider, api_key)?;
            client
                .get(format!("{base}/v1beta/models"))
                .header("x-goog-api-key", key.expose())
        }
    };

    let resp = req.send().await.map_err(|e| {
        if provider == LlmProvider::Ollama {
            CoreError::Network(format!(
                "Ollama non raggiungibile su {base}: verifica che Ollama sia in esecuzione ({e})"
            ))
        } else {
            CoreError::Network(format!(
                "{}: errore di rete: {}",
                provider.display_name(),
                sanitize_error_snippet(&e.to_string(), api_key)
            ))
        }
    })?;

    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(map_provider_http_error(
            provider,
            status,
            &text,
            retry_after.as_deref(),
            api_key,
        ));
    }

    let json_val: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Network(format!("Invalid model list JSON: {e}")))?;

    Ok(parse_models_payload(provider, &json_val))
}

fn parse_models_payload(provider: LlmProvider, v: &Value) -> Vec<LlmModelInfo> {
    let mut out = Vec::new();
    match provider {
        LlmProvider::Ollama => {
            if let Some(arr) = v.get("models").and_then(Value::as_array) {
                for item in arr {
                    if let Some(name) = item
                        .get("name")
                        .or_else(|| item.get("model"))
                        .and_then(Value::as_str)
                    {
                        out.push(LlmModelInfo {
                            id: name.to_string(),
                            display_name: name.to_string(),
                        });
                    }
                }
            }
        }
        LlmProvider::Anthropic | LlmProvider::OpenAi => {
            if let Some(arr) = v.get("data").and_then(Value::as_array) {
                for item in arr {
                    if let Some(id) = item.get("id").and_then(Value::as_str) {
                        let label = item
                            .get("display_name")
                            .and_then(Value::as_str)
                            .unwrap_or(id);
                        out.push(LlmModelInfo {
                            id: id.to_string(),
                            display_name: label.to_string(),
                        });
                    }
                }
            }
        }
        LlmProvider::Gemini => {
            if let Some(arr) = v.get("models").and_then(Value::as_array) {
                for item in arr {
                    let supports_gen = item
                        .get("supportedGenerationMethods")
                        .and_then(Value::as_array)
                        .is_none_or(|methods| {
                            methods
                                .iter()
                                .any(|m| m.as_str() == Some("generateContent"))
                        });
                    if !supports_gen {
                        continue;
                    }
                    if let Some(raw_name) = item.get("name").and_then(Value::as_str) {
                        let id = raw_name.strip_prefix("models/").unwrap_or(raw_name);
                        let label = item
                            .get("displayName")
                            .and_then(Value::as_str)
                            .unwrap_or(id);
                        out.push(LlmModelInfo {
                            id: id.to_string(),
                            display_name: format!("{label} ({id})"),
                        });
                    }
                }
            }
        }
        LlmProvider::Disabled => {}
    }

    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Verifies API key / server reachability via live `list_models` without billing tokens.
pub async fn verify_provider_key(
    provider: LlmProvider,
    api_key: Option<&SecretApiKey>,
    ollama_url: Option<&str>,
    base_url_override: Option<&str>,
) -> LlmKeyVerifyResult {
    match list_models(provider, api_key, ollama_url, base_url_override).await {
        Ok(models) => LlmKeyVerifyResult {
            ok: true,
            provider,
            models_count: models.len(),
            message: format!(
                "{}: connessione riuscita ({} modelli disponibili).",
                provider.display_name(),
                models.len()
            ),
            models,
        },
        Err(e) => LlmKeyVerifyResult {
            ok: false,
            provider,
            models_count: 0,
            models: Vec::new(),
            message: e.to_string(),
        },
    }
}

/// Generates a grounded response strictly on-demand over retrieved passages, enforcing
/// privacy consent, per-doc/per-folder exclusions, token budgets, and clickable `[Doc #id, p. X]` citations.
/// Reads cloud exclusions synchronously before any network `.await` so DB locks never cross await points.
#[allow(clippy::too_many_arguments)]
pub async fn generate_grounded_answer(
    db: Option<&Database>,
    config: &AppConfig,
    api_key: Option<&SecretApiKey>,
    query: &str,
    results: &[ContentSearchDocResult],
    max_passages_override: Option<usize>,
    max_input_tokens_override: Option<usize>,
    base_url_override: Option<&str>,
) -> Result<LlmAnswerResult> {
    let prepared = {
        let reader_guard = db.map(Database::reader);
        prepare_grounded_prompt(
            reader_guard.as_deref(),
            config,
            query,
            results,
            max_passages_override,
            max_input_tokens_override,
        )?
    };
    send_prepared_prompt(config, api_key, prepared, base_url_override).await
}

/// Dispatches an already-filtered [`PreparedPrompt`] to the active LLM provider without
/// holding any database lock across `.await`.
pub async fn send_prepared_prompt(
    config: &AppConfig,
    api_key: Option<&SecretApiKey>,
    prepared: PreparedPrompt,
    base_url_override: Option<&str>,
) -> Result<LlmAnswerResult> {
    let provider = config.llm_provider;
    if !provider.is_enabled() {
        return Err(CoreError::Config(
            "LLM provider is disabled in settings.".to_string(),
        ));
    }

    let model = config
        .llm_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CoreError::Config(
                "No LLM model selected. Fetch models and choose one in Settings.".to_string(),
            )
        })?
        .to_string();

    if provider.is_cloud() && !has_cloud_consent(config, provider) {
        return Err(CoreError::Config(
            "Cloud LLM privacy consent required before first transmission.".to_string(),
        ));
    }

    // Short-circuit BEFORE making any HTTP request when zero non-excluded passages exist.
    if prepared.selected_passages.is_empty() {
        let is_en = config.ui_language.trim().eq_ignore_ascii_case("en");
        let answer_markdown = if is_en {
            REFUSAL_EN.to_string()
        } else {
            REFUSAL_IT.to_string()
        };
        let reason = if prepared.preview.passages_excluded_cloud > 0 {
            Some("all retrieved passages excluded by cloud privacy rules".to_string())
        } else {
            Some("no passages retrieved for query".to_string())
        };
        return Ok(LlmAnswerResult {
            provider,
            model,
            answer_markdown,
            citations: Vec::new(),
            refused: true,
            refusal_reason: reason,
            usage: LlmUsage::default(),
            estimated_input_tokens: 0,
            included_passages: 0,
            excluded_passages: prepared.preview.passages_excluded_cloud,
        });
    }

    let (answer_text, usage) = invoke_provider_chat(
        provider,
        &model,
        api_key,
        Some(&config.llm_ollama_url),
        base_url_override,
        &prepared,
        config.llm_max_output_tokens.clamp(128, 8192),
    )
    .await?;

    let refused = detect_refusal(&answer_text);
    let citations = if refused {
        Vec::new()
    } else {
        parse_citations(&answer_text, &prepared.selected_passages)
    };

    Ok(LlmAnswerResult {
        provider,
        model,
        answer_markdown: answer_text,
        citations,
        refused,
        refusal_reason: if refused {
            Some("passages insufficient to answer query".to_string())
        } else {
            None
        },
        usage,
        estimated_input_tokens: prepared.preview.estimated_input_tokens,
        included_passages: prepared.preview.passages_included,
        excluded_passages: prepared.preview.passages_excluded_cloud,
    })
}

async fn invoke_provider_chat(
    provider: LlmProvider,
    model: &str,
    api_key: Option<&SecretApiKey>,
    ollama_url: Option<&str>,
    base_url_override: Option<&str>,
    prepared: &PreparedPrompt,
    max_output_tokens: usize,
) -> Result<(String, LlmUsage)> {
    let base = resolve_base_url(provider, ollama_url, base_url_override);
    let is_local = base.starts_with("http://127.0.0.1") || base.starts_with("http://localhost");
    let client = build_http_client(is_local)?;

    let req = match provider {
        LlmProvider::Disabled => unreachable!(),
        LlmProvider::Ollama => {
            let body = json!({
                "model": model,
                "stream": false,
                "options": {
                    "num_predict": max_output_tokens,
                    "temperature": 0.1
                },
                "messages": [
                    { "role": "system", "content": prepared.system_prompt },
                    { "role": "user", "content": prepared.user_prompt }
                ]
            });
            client.post(format!("{base}/api/chat")).json(&body)
        }
        LlmProvider::Anthropic => {
            let key = require_cloud_key(provider, api_key)?;
            let body = json!({
                "model": model,
                "max_tokens": max_output_tokens,
                "temperature": 0.1,
                "system": prepared.system_prompt,
                "messages": [
                    { "role": "user", "content": prepared.user_prompt }
                ]
            });
            client
                .post(format!("{base}/v1/messages"))
                .header("x-api-key", key.expose())
                .header("anthropic-version", "2023-06-01")
                .json(&body)
        }
        LlmProvider::OpenAi => {
            let key = require_cloud_key(provider, api_key)?;
            let body = json!({
                "model": model,
                "max_tokens": max_output_tokens,
                "temperature": 0.1,
                "messages": [
                    { "role": "system", "content": prepared.system_prompt },
                    { "role": "user", "content": prepared.user_prompt }
                ]
            });
            client
                .post(format!("{base}/v1/chat/completions"))
                .bearer_auth(key.expose())
                .json(&body)
        }
        LlmProvider::Gemini => {
            let key = require_cloud_key(provider, api_key)?;
            let clean_model = model.strip_prefix("models/").unwrap_or(model);
            let body = json!({
                "systemInstruction": {
                    "parts": [{ "text": prepared.system_prompt }]
                },
                "contents": [
                    {
                        "role": "user",
                        "parts": [{ "text": prepared.user_prompt }]
                    }
                ],
                "generationConfig": {
                    "temperature": 0.1,
                    "maxOutputTokens": max_output_tokens
                }
            });
            client
                .post(format!("{base}/v1beta/models/{clean_model}:generateContent"))
                .header("x-goog-api-key", key.expose())
                .json(&body)
        }
    };

    let resp = req.send().await.map_err(|e| {
        CoreError::Network(format!(
            "{}: errore di rete: {}",
            provider.display_name(),
            sanitize_error_snippet(&e.to_string(), api_key)
        ))
    })?;

    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    let body_text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(map_provider_http_error(
            provider,
            status,
            &body_text,
            retry_after.as_deref(),
            api_key,
        ));
    }

    let val: Value = serde_json::from_str(&body_text)
        .map_err(|e| CoreError::Network(format!("Invalid provider response JSON: {e}")))?;

    parse_chat_response(provider, &val)
}

fn parse_chat_response(provider: LlmProvider, v: &Value) -> Result<(String, LlmUsage)> {
    match provider {
        LlmProvider::Disabled => unreachable!(),
        LlmProvider::Ollama => {
            let text = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .or_else(|| v.get("response").and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            let input = v.get("prompt_eval_count").and_then(Value::as_u64);
            let output = v.get("eval_count").and_then(Value::as_u64);
            let total = input.zip(output).map(|(a, b)| a + b);
            Ok((
                text,
                LlmUsage {
                    input_tokens: input,
                    output_tokens: output,
                    total_tokens: total,
                },
            ))
        }
        LlmProvider::Anthropic => {
            let text = v
                .get("content")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
                .trim()
                .to_string();
            let usage = v.get("usage");
            let input = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(Value::as_u64);
            let output = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64);
            let total = input.zip(output).map(|(a, b)| a + b);
            Ok((
                text,
                LlmUsage {
                    input_tokens: input,
                    output_tokens: output,
                    total_tokens: total,
                },
            ))
        }
        LlmProvider::OpenAi => {
            let text = v
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let usage = v.get("usage");
            let input = usage
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(Value::as_u64);
            let output = usage
                .and_then(|u| u.get("completion_tokens"))
                .and_then(Value::as_u64);
            let total = usage
                .and_then(|u| u.get("total_tokens"))
                .and_then(Value::as_u64)
                .or_else(|| input.zip(output).map(|(a, b)| a + b));
            Ok((
                text,
                LlmUsage {
                    input_tokens: input,
                    output_tokens: output,
                    total_tokens: total,
                },
            ))
        }
        LlmProvider::Gemini => {
            let text = v
                .get("candidates")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("content"))
                .and_then(|ct| ct.get("parts"))
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
                .trim()
                .to_string();
            let usage = v.get("usageMetadata");
            let input = usage
                .and_then(|u| u.get("promptTokenCount"))
                .and_then(Value::as_u64);
            let output = usage
                .and_then(|u| u.get("candidatesTokenCount"))
                .and_then(Value::as_u64);
            let total = usage
                .and_then(|u| u.get("totalTokenCount"))
                .and_then(Value::as_u64)
                .or_else(|| input.zip(output).map(|(a, b)| a + b));
            Ok((
                text,
                LlmUsage {
                    input_tokens: input,
                    output_tokens: output,
                    total_tokens: total,
                },
            ))
        }
    }
}
