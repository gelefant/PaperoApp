use crate::state::AppState;
use papero_core::credentials::{delete_secret_key, load_secret_key, save_secret_key, SecretApiKey};
use papero_core::db::repos::docs::ContentSearchDocResult;
use papero_core::llm::{
    accept_cloud_consent, convert_snip_image_to_latex, generate_grounded_answer,
    generate_grounded_answer_streaming, list_models, prepare_document_scoped_prompt,
    prepare_grounded_prompt, send_prepared_prompt_streaming, verify_provider_key, DocLlmScope,
    LlmAnswerResult, LlmKeyVerifyResult, LlmModelInfo, LlmPromptPreview, LlmProvider,
    LlmStreamEvent, SnipLatexResult,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
pub struct LlmKeyStatus {
    pub anthropic_present: bool,
    pub openai_present: bool,
    pub gemini_present: bool,
}

fn load_provider_key_from_keyring(provider: LlmProvider) -> Result<Option<SecretApiKey>, String> {
    match provider.keyring_account_key() {
        Some(acct) => load_secret_key(acct).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

fn resolve_effective_llm_key(
    provider: LlmProvider,
    api_key_override: Option<String>,
) -> Result<Option<SecretApiKey>, String> {
    if !provider.is_cloud() {
        return Ok(None);
    }
    if let Some(raw) = api_key_override {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(Some(SecretApiKey::new(trimmed.to_string())));
        }
    }
    load_provider_key_from_keyring(provider)
}

#[tauri::command]
pub async fn get_llm_key_status() -> Result<LlmKeyStatus, String> {
    tokio::task::spawn_blocking(|| LlmKeyStatus {
        anthropic_present: load_provider_key_from_keyring(LlmProvider::Anthropic)
            .ok()
            .flatten()
            .is_some(),
        openai_present: load_provider_key_from_keyring(LlmProvider::OpenAi)
            .ok()
            .flatten()
            .is_some(),
        gemini_present: load_provider_key_from_keyring(LlmProvider::Gemini)
            .ok()
            .flatten()
            .is_some(),
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_llm_api_key(provider: LlmProvider, api_key: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let acct = provider
            .keyring_account_key()
            .ok_or_else(|| "Selected provider does not use a cloud API key.".to_string())?;
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            return Err("API key cannot be empty.".to_string());
        }
        let secret = SecretApiKey::new(trimmed.to_string());
        save_secret_key(acct, &secret).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn remove_llm_api_key(provider: LlmProvider) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let acct = provider
            .keyring_account_key()
            .ok_or_else(|| "Selected provider does not use a cloud API key.".to_string())?;
        delete_secret_key(acct).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn verify_llm_key_cmd(
    state: State<'_, Arc<AppState>>,
    provider: LlmProvider,
    api_key_override: Option<String>,
    ollama_url_override: Option<String>,
) -> Result<LlmKeyVerifyResult, String> {
    let ollama_url = ollama_url_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.config.read().expect("config poisoned").llm_ollama_url.clone());
    let key = resolve_effective_llm_key(provider, api_key_override)?;
    Ok(verify_provider_key(provider, key.as_ref(), Some(&ollama_url), None).await)
}

#[tauri::command]
pub async fn list_llm_models_cmd(
    state: State<'_, Arc<AppState>>,
    provider: LlmProvider,
    ollama_url_override: Option<String>,
    api_key_override: Option<String>,
) -> Result<Vec<LlmModelInfo>, String> {
    let ollama_url = ollama_url_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.config.read().expect("config poisoned").llm_ollama_url.clone());
    let key = resolve_effective_llm_key(provider, api_key_override)?;
    list_models(provider, key.as_ref(), Some(&ollama_url), None)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_llm_prompt_cmd(
    state: State<'_, Arc<AppState>>,
    query: String,
    results: Vec<ContentSearchDocResult>,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
) -> Result<LlmPromptPreview, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let reader = st.db.reader();
        let prepared = prepare_grounded_prompt(
            Some(&reader),
            &cfg,
            &query,
            &results,
            max_passages,
            max_input_tokens,
        )
        .map_err(|e| e.to_string())?;
        Ok(prepared.preview)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn accept_llm_cloud_consent_cmd(
    state: State<'_, Arc<AppState>>,
    provider: LlmProvider,
) -> Result<(), String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let mut cfg = st.config.write().expect("config poisoned");
        accept_cloud_consent(&mut cfg, provider);
        cfg.save(&st.paths.config_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn generate_llm_answer_cmd(
    state: State<'_, Arc<AppState>>,
    query: String,
    results: Vec<ContentSearchDocResult>,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
) -> Result<LlmAnswerResult, String> {
    let st = Arc::clone(&state);
    let cfg = st.config.read().expect("config poisoned").clone();
    let key = if cfg.llm_provider.is_cloud() {
        load_provider_key_from_keyring(cfg.llm_provider)?
    } else {
        None
    };
    generate_grounded_answer(
        Some(&st.db),
        &cfg,
        key.as_ref(),
        &query,
        &results,
        max_passages,
        max_input_tokens,
        None,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_llm_answer_stream_cmd(
    state: State<'_, Arc<AppState>>,
    query: String,
    results: Vec<ContentSearchDocResult>,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
    on_event: Channel<LlmStreamEvent>,
) -> Result<LlmAnswerResult, String> {
    let st = Arc::clone(&state);
    let cfg = st.config.read().expect("config poisoned").clone();
    let key = if cfg.llm_provider.is_cloud() {
        load_provider_key_from_keyring(cfg.llm_provider)?
    } else {
        None
    };
    let ev_channel = on_event.clone();
    let res = generate_grounded_answer_streaming(
        Some(&st.db),
        &cfg,
        key.as_ref(),
        &query,
        &results,
        max_passages,
        max_input_tokens,
        None,
        move |delta| {
            let _ = ev_channel.send(LlmStreamEvent::Delta {
                text: delta.to_string(),
            });
        },
    )
    .await;
    match res {
        Ok(ans) => {
            let _ = on_event.send(LlmStreamEvent::Done {
                result: ans.clone(),
            });
            Ok(ans)
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = on_event.send(LlmStreamEvent::Error {
                message: msg.clone(),
            });
            Err(msg)
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn preview_doc_llm_prompt_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    file_id: i64,
    scope: DocLlmScope,
    page_ord: u32,
    selection_text: Option<String>,
    question: String,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
) -> Result<LlmPromptPreview, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let r = st.db.reader();
        let prep = prepare_document_scoped_prompt(
            &r,
            &cfg,
            document_id,
            file_id,
            scope,
            page_ord,
            selection_text.as_deref(),
            &question,
            max_passages,
            max_input_tokens,
        )
        .map_err(|e| e.to_string())?;
        Ok(prep.preview)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ask_document_llm_stream_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    file_id: i64,
    scope: DocLlmScope,
    page_ord: u32,
    selection_text: Option<String>,
    question: String,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
    on_event: Channel<LlmStreamEvent>,
) -> Result<LlmAnswerResult, String> {
    let st = Arc::clone(&state);
    let cfg = st.config.read().expect("config poisoned").clone();
    let key = if cfg.llm_provider.is_cloud() {
        load_provider_key_from_keyring(cfg.llm_provider)?
    } else {
        None
    };

    let cfg_prep = cfg.clone();
    let db_prep = Arc::clone(&st.db);
    let prepared = tokio::task::spawn_blocking(move || {
        let r = db_prep.reader();
        prepare_document_scoped_prompt(
            &r,
            &cfg_prep,
            document_id,
            file_id,
            scope,
            page_ord,
            selection_text.as_deref(),
            &question,
            max_passages,
            max_input_tokens,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let ev_channel = on_event.clone();
    let res = send_prepared_prompt_streaming(&cfg, key.as_ref(), prepared, None, move |delta| {
        let _ = ev_channel.send(LlmStreamEvent::Delta {
            text: delta.to_string(),
        });
    })
    .await;

    match res {
        Ok(ans) => {
            let _ = on_event.send(LlmStreamEvent::Done {
                result: ans.clone(),
            });
            Ok(ans)
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = on_event.send(LlmStreamEvent::Error {
                message: msg.clone(),
            });
            Err(msg)
        }
    }
}

#[tauri::command]
pub async fn snip_image_to_latex_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    image_data_url: String,
    hint: Option<String>,
) -> Result<SnipLatexResult, String> {
    let st = Arc::clone(&state);
    let cfg = st.config.read().expect("config poisoned").clone();
    let key = if cfg.llm_provider.is_cloud() {
        load_provider_key_from_keyring(cfg.llm_provider)?
    } else {
        None
    };
    convert_snip_image_to_latex(
        Some(&st.db),
        &cfg,
        key.as_ref(),
        document_id,
        &image_data_url,
        hint.as_deref(),
        None,
    )
    .await
    .map_err(|e| e.to_string())
}
