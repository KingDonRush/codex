use codex_model_provider_info::ANTHROPIC_DEFAULT_MODEL_ID;
use codex_models_manager::model_info::BASE_INSTRUCTIONS;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;

const CLAUDE_CONTEXT_WINDOW: i64 = 200_000;
const CLAUDE_DEFAULT_MAX_OUTPUT_TOKENS: i64 = 4096;

pub(crate) fn static_model_catalog() -> ModelsResponse {
    ModelsResponse {
        models: vec![
            claude_model(
                ANTHROPIC_DEFAULT_MODEL_ID,
                "Claude Sonnet 4.6",
                "Best combination of speed and intelligence.",
                /*priority*/ 0,
            ),
            claude_model(
                "claude-opus-4-7",
                "Claude Opus 4.7",
                "Most capable Claude model for complex reasoning and agentic coding.",
                /*priority*/ 1,
            ),
            claude_model(
                "claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                "Fast Claude model with near-frontier intelligence.",
                /*priority*/ 2,
            ),
        ],
    }
}

fn claude_model(slug: &str, display_name: &str, description: &str, priority: i32) -> ModelInfo {
    ModelInfo {
        slug: slug.to_string(),
        display_name: display_name.to_string(),
        description: Some(description.to_string()),
        default_reasoning_level: Some(ReasoningEffort::None),
        supported_reasoning_levels: vec![reasoning_effort_preset(ReasoningEffort::None)],
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: BASE_INSTRUCTIONS.to_string(),
        model_messages: None,
        supports_reasoning_summaries: false,
        default_reasoning_summary: ReasoningSummary::None,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None::<ApplyPatchToolType>,
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::tokens(/*limit*/ 10_000),
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: Some(CLAUDE_CONTEXT_WINDOW),
        max_context_window: Some(CLAUDE_CONTEXT_WINDOW),
        max_output_tokens: Some(CLAUDE_DEFAULT_MAX_OUTPUT_TOKENS),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: vec![InputModality::Text],
        used_fallback_model_metadata: false,
        supports_search_tool: true,
    }
}

fn reasoning_effort_preset(effort: ReasoningEffort) -> ReasoningEffortPreset {
    ReasoningEffortPreset {
        effort,
        description: "No provider-native reasoning control".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn catalog_uses_sonnet_as_default_model() {
        let catalog = static_model_catalog();

        assert_eq!(catalog.models.len(), 3);
        assert_eq!(catalog.models[0].slug, ANTHROPIC_DEFAULT_MODEL_ID);
        assert_eq!(
            catalog.models[0].shell_type,
            ConfigShellToolType::ShellCommand
        );
        assert_eq!(catalog.models[0].apply_patch_tool_type, None);
        assert!(catalog.models[0].supports_search_tool);
    }
}
