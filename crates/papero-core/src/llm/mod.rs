pub mod citations;
pub mod client;
pub mod guard;
pub mod provider;

pub use citations::parse_citations;
pub use client::{
    generate_grounded_answer, list_models, map_provider_http_error, send_prepared_prompt,
    verify_provider_key,
};
pub use guard::{
    accept_cloud_consent, build_grounding_system_prompt, detect_refusal, estimate_tokens,
    has_cloud_consent, prepare_grounded_prompt, query_cloud_exclusion_map, PreparedPrompt,
    SelectedPassage, REFUSAL_EN, REFUSAL_IT,
};
pub use provider::{
    LlmAnswerResult, LlmCitation, LlmKeyVerifyResult, LlmModelInfo, LlmPromptPreview, LlmProvider,
    LlmUsage,
};
