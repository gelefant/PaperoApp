use crate::config::AppConfig;
use crate::db::repos::docs::ContentSearchDocResult;
use crate::error::Result;
use crate::extract::types::NormalizedRect;
use crate::llm::provider::{LlmPromptPreview, LlmProvider};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

pub const REFUSAL_IT: &str =
    "I passaggi trovati non contengono informazioni sufficienti per rispondere alla domanda.";
pub const REFUSAL_EN: &str =
    "The retrieved passages do not contain enough information to answer the question.";

#[derive(Debug, Clone)]
pub struct SelectedPassage {
    pub passage_id: u64,
    pub document_id: i64,
    pub file_id: i64,
    pub doc_title: String,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub score: f32,
    pub text_content: String,
    pub bboxes: Vec<NormalizedRect>,
}

#[derive(Debug, Clone)]
pub struct PreparedPrompt {
    pub preview: LlmPromptPreview,
    pub system_prompt: String,
    pub user_prompt: String,
    pub selected_passages: Vec<SelectedPassage>,
}

/// Conservative token estimator suitable for mathematical prose + LaTeX symbols (`~chars/3`).
pub fn estimate_tokens(text: &str) -> usize {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let char_bound = trimmed.chars().count().div_ceil(3);
    let word_bound = trimmed.split_whitespace().count();
    char_bound.max(word_bound)
}

pub fn has_cloud_consent(config: &AppConfig, provider: LlmProvider) -> bool {
    match provider {
        LlmProvider::Disabled | LlmProvider::Ollama => true,
        LlmProvider::Anthropic => config.llm_consent_anthropic,
        LlmProvider::OpenAi => config.llm_consent_openai,
        LlmProvider::Gemini => config.llm_consent_gemini,
    }
}

pub fn accept_cloud_consent(config: &mut AppConfig, provider: LlmProvider) {
    match provider {
        LlmProvider::Anthropic => config.llm_consent_anthropic = true,
        LlmProvider::OpenAi => config.llm_consent_openai = true,
        LlmProvider::Gemini => config.llm_consent_gemini = true,
        LlmProvider::Disabled | LlmProvider::Ollama => {}
    }
}

/// Queries SQLite for whether each document is excluded from cloud transmission either
/// directly (`documents.exclude_from_cloud_llm`) OR via any of its source folders
/// (`source_roots.exclude_from_cloud_llm`).
pub fn query_cloud_exclusion_map(
    conn: &Connection,
    doc_ids: &[i64],
) -> Result<HashMap<i64, bool>> {
    let mut map = HashMap::new();
    if doc_ids.is_empty() {
        return Ok(map);
    }

    let placeholders = (1..=doc_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT d.id,
                (d.exclude_from_cloud_llm != 0 OR COALESCE(MAX(sr.exclude_from_cloud_llm), 0) != 0) AS excluded
         FROM documents d
         LEFT JOIN files f ON f.document_id = d.id
         LEFT JOIN source_roots sr ON sr.id = f.root_id
         WHERE d.id IN ({placeholders})
         GROUP BY d.id"
    );

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> =
        doc_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(refs.as_slice(), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0))
    })?;
    for row in rows {
        let (doc_id, excluded) = row?;
        map.insert(doc_id, excluded);
    }
    Ok(map)
}

pub fn build_grounding_system_prompt(ui_language: &str) -> String {
    let is_en = ui_language.trim().eq_ignore_ascii_case("en");
    let refusal = if is_en { REFUSAL_EN } else { REFUSAL_IT };
    if is_en {
        format!(
            "You are a precise mathematical research assistant embedded in Papero. \
             Answer the user's question using ONLY facts, definitions, formulas, and theorems \
             explicitly stated in the provided document passages.\n\
             Strict rules:\n\
             1. Cite every claim inline using the exact bracket format `[Doc #<doc_id>, p. <page_ord>]` \
                matching the passage headers.\n\
             2. Do NOT draw from outside training knowledge, do NOT extrapolate unverified proofs, \
                and do NOT invent document numbers or page numbers.\n\
             3. If the provided passages do not contain enough information to answer the question, \
                respond strictly with: \"{refusal}\""
        )
    } else {
        format!(
            "Sei un assistente di ricerca matematica integrato in Papero. \
             Rispondi alla domanda dell'utente basandoti ESCLUSIVAMENTE sui fatti, definizioni, \
             formule e teoremi presenti nei passaggi forniti.\n\
             Regole rigorose:\n\
             1. Indica per ogni affermazione il passaggio di provenienza nel formato esatto `[Doc #<doc_id>, p. <page_ord>]`.\n\
             2. NON usare conoscenze esterne ai passaggi forniti, NON completare dimostrazioni non presenti e NON inventare numeri di documento o pagina.\n\
             3. Se i passaggi trovati non contengono informazioni sufficienti per rispondere, \
                rispondi esattamente con: \"{refusal}\""
        )
    }
}

pub fn detect_refusal(answer: &str) -> bool {
    let lower = answer.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    lower.contains("non contengono informazioni sufficienti")
        || lower.contains("do not contain enough information")
        || lower.contains("does not contain enough information")
        || lower.contains("insufficient information in the provided passages")
}

/// Applies cloud exclusion filters (when sending to Anthropic/OpenAI/Gemini), packs passages
/// strictly within `max_passages` and `max_input_tokens`, and computes pre-send token estimates.
pub fn prepare_grounded_prompt(
    conn: Option<&Connection>,
    config: &AppConfig,
    query: &str,
    results: &[ContentSearchDocResult],
    max_passages_override: Option<usize>,
    max_input_tokens_override: Option<usize>,
) -> Result<PreparedPrompt> {
    let provider = config.llm_provider;
    let max_passages = max_passages_override
        .unwrap_or(config.llm_max_passages)
        .clamp(1, 40);
    let max_input_tokens = max_input_tokens_override
        .unwrap_or(config.llm_max_input_tokens)
        .clamp(256, 64_000);

    let doc_ids: Vec<i64> = {
        let mut seen = HashSet::new();
        results
            .iter()
            .filter_map(|d| seen.insert(d.document_id).then_some(d.document_id))
            .collect()
    };

    let exclusion_map = match (provider.is_cloud(), conn) {
        (true, Some(c)) => query_cloud_exclusion_map(c, &doc_ids)?,
        _ => HashMap::new(),
    };

    // Flatten and rank individual passages across documents by retrieval score descending.
    let mut candidates: Vec<(SelectedPassage, bool)> = Vec::new();
    for doc in results {
        let is_excluded = provider.is_cloud()
            && *exclusion_map.get(&doc.document_id).unwrap_or(&false);
        for p in &doc.passages {
            candidates.push((
                SelectedPassage {
                    passage_id: p.passage_id,
                    document_id: doc.document_id,
                    file_id: p.file_id,
                    doc_title: doc.title.clone(),
                    page_ord: p.page_ord,
                    printed_label: p.printed_label.clone(),
                    score: p.score,
                    text_content: p.text_content.clone(),
                    bboxes: p.bboxes.clone(),
                },
                is_excluded,
            ));
        }
    }
    candidates.sort_by(|a, b| {
        b.0.score
            .partial_cmp(&a.0.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let passages_total = candidates.len();
    let mut passages_excluded_cloud = 0usize;
    let mut excluded_titles_set = HashSet::new();
    let mut excluded_doc_titles = Vec::new();
    let mut allowed: Vec<SelectedPassage> = Vec::new();

    for (passage, excluded) in candidates {
        if excluded {
            passages_excluded_cloud += 1;
            if excluded_titles_set.insert(passage.doc_title.clone()) {
                excluded_doc_titles.push(passage.doc_title);
            }
        } else {
            allowed.push(passage);
        }
    }

    let system_prompt = build_grounding_system_prompt(&config.ui_language);
    let query_header = format!("User query:\n{}\n\nPassages:\n", query.trim());
    let base_tokens = estimate_tokens(&system_prompt) + estimate_tokens(&query_header) + 16;

    let mut running_tokens = base_tokens;
    let mut selected_passages = Vec::new();
    let mut blocks = Vec::new();
    let mut passages_dropped_budget = 0usize;

    for (idx, mut passage) in allowed.into_iter().enumerate() {
        if idx >= max_passages {
            passages_dropped_budget += 1;
            continue;
        }
        let header = format!(
            "[Doc #{}, p. {} | \"{}\"]\n",
            passage.document_id, passage.page_ord, passage.doc_title
        );
        let full_block = format!("{header}{}", passage.text_content.trim());
        let block_tokens = estimate_tokens(&full_block) + 4;

        if running_tokens + block_tokens <= max_input_tokens {
            running_tokens += block_tokens;
            blocks.push(full_block);
            selected_passages.push(passage);
        } else if selected_passages.is_empty() && max_input_tokens > base_tokens + 40 {
            // Fit the top passage safely truncated within max_input_tokens.
            let remaining_tokens = max_input_tokens.saturating_sub(base_tokens + estimate_tokens(&header) + 10);
            let max_chars = remaining_tokens.saturating_mul(3);
            let truncated: String = passage.text_content.trim().chars().take(max_chars).collect();
            if !truncated.trim().is_empty() {
                passage.text_content = format!("{truncated}…");
                let trunc_block = format!("{header}{}", passage.text_content);
                running_tokens = base_tokens + estimate_tokens(&trunc_block) + 4;
                blocks.push(trunc_block);
                selected_passages.push(passage);
            } else {
                passages_dropped_budget += 1;
            }
        } else {
            passages_dropped_budget += 1;
        }
    }

    let user_prompt = format!("{query_header}{}", blocks.join("\n\n---\n\n"));

    let preview = LlmPromptPreview {
        provider,
        model: config.llm_model.clone(),
        requires_cloud_consent: !has_cloud_consent(config, provider),
        estimated_input_tokens: running_tokens,
        max_input_tokens,
        max_passages,
        passages_total,
        passages_included: selected_passages.len(),
        passages_excluded_cloud,
        passages_dropped_budget,
        excluded_doc_titles,
    };

    Ok(PreparedPrompt {
        preview,
        system_prompt,
        user_prompt,
        selected_passages,
    })
}
