use crate::extract::types::NormalizedRect;
use serde::{Deserialize, Serialize};

/// Supported optional generative LLM backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    #[default]
    Disabled,
    Ollama,
    Anthropic,
    OpenAi,
    Gemini,
}

impl LlmProvider {
    pub fn parse_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ollama" => Self::Ollama,
            "anthropic" | "claude" => Self::Anthropic,
            "openai" | "open_ai" => Self::OpenAi,
            "gemini" | "google" => Self::Gemini,
            _ => Self::Disabled,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ollama => "ollama",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
        }
    }

    pub fn is_enabled(self) -> bool {
        self != Self::Disabled
    }

    pub fn is_cloud(self) -> bool {
        matches!(self, Self::Anthropic | Self::OpenAi | Self::Gemini)
    }

    /// Keyring account name inside service `"org.papero.app"`.
    pub fn keyring_account_key(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("llm_key_anthropic"),
            Self::OpenAi => Some("llm_key_openai"),
            Self::Gemini => Some("llm_key_gemini"),
            Self::Disabled | Self::Ollama => None,
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Disabled | Self::Ollama => "http://127.0.0.1:11434",
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.openai.com",
            Self::Gemini => "https://generativelanguage.googleapis.com",
        }
    }

    pub fn api_key_console_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("https://console.anthropic.com/settings/keys"),
            Self::OpenAi => Some("https://platform.openai.com/api-keys"),
            Self::Gemini => Some("https://aistudio.google.com/apikey"),
            Self::Disabled | Self::Ollama => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Ollama => "Ollama (Local)",
            Self::Anthropic => "Anthropic (Claude API)",
            Self::OpenAi => "OpenAI API",
            Self::Gemini => "Google Gemini API",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmModelInfo {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmPromptPreview {
    pub provider: LlmProvider,
    pub model: Option<String>,
    pub requires_cloud_consent: bool,
    pub estimated_input_tokens: usize,
    pub max_input_tokens: usize,
    pub max_passages: usize,
    pub passages_total: usize,
    pub passages_included: usize,
    pub passages_excluded_cloud: usize,
    pub passages_dropped_budget: usize,
    pub excluded_doc_titles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmCitation {
    pub marker: String,
    pub document_id: i64,
    pub file_id: i64,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub doc_title: String,
    pub bboxes: Vec<NormalizedRect>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmAnswerResult {
    pub provider: LlmProvider,
    pub model: String,
    pub answer_markdown: String,
    pub citations: Vec<LlmCitation>,
    pub refused: bool,
    pub refusal_reason: Option<String>,
    pub usage: LlmUsage,
    pub estimated_input_tokens: usize,
    pub included_passages: usize,
    pub excluded_passages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmKeyVerifyResult {
    pub ok: bool,
    pub provider: LlmProvider,
    pub models_count: usize,
    pub models: Vec<LlmModelInfo>,
    pub message: String,
}
