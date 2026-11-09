use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    text_only_model_pattern: String,
    image_input_model_pattern: String,
    cloudflare_workers_ai_models: Vec<Model>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    model: String,
    input_modalities: Vec<String>,
}

static POLICY: LazyLock<(Policy, Regex, Regex)> = LazyLock::new(|| {
    let policy: Policy = serde_json::from_str(include_str!(
        "../../../../assets/local-image-capabilities.json"
    ))
    .expect("committed Local image capability policy must be valid");
    let compile = |pattern: &str| {
        RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .expect("committed Local image capability pattern must be valid")
    };
    let text_only = compile(&policy.text_only_model_pattern);
    let image_input = compile(&policy.image_input_model_pattern);
    (policy, text_only, image_input)
});

pub(super) fn supports_images(provider: &str, model: &str) -> bool {
    if provider.is_empty() || model.is_empty() || provider == "chatgpt_subscription" {
        return false;
    }
    let (policy, text_only, image_input) = &*POLICY;
    if provider == "cloudflare_workers_ai"
        && let Some(entry) = policy
            .cloudflare_workers_ai_models
            .iter()
            .find(|entry| entry.model == model)
    {
        return entry.input_modalities.iter().any(|value| value == "image");
    }
    !text_only.is_match(model) && image_input.is_match(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_image_capability_matches_legacy_known_and_unknown_policy() {
        for (provider, model, expected) in [
            ("openai", "gpt-4o", true),
            ("openai", "gpt-4", false),
            ("openai", "gpt-3.5-turbo", false),
            ("anthropic", "claude-3-5-sonnet", true),
            ("anthropic", "claude-2.1", false),
            ("google_generative_ai", "gemini-2.5-pro", true),
            ("ollama", "llava:latest", true),
            ("ollama", "llama-3.1-8b", false),
            ("custom", "unknown-model", false),
            ("custom", "embedding-gpt-4o", false),
            ("custom", "QWEN2.5-VL", true),
            ("chatgpt_subscription", "gpt-4o", false),
            ("", "gpt-4o", false),
            ("openai", "", false),
        ] {
            assert_eq!(
                supports_images(provider, model),
                expected,
                "{provider}/{model}"
            );
        }
    }

    #[test]
    fn cloudflare_catalog_metadata_precedes_name_rules_without_duplicate_models() {
        let mut seen = std::collections::HashSet::new();
        for entry in &POLICY.0.cloudflare_workers_ai_models {
            assert!(seen.insert(&entry.model));
            assert_eq!(
                supports_images("cloudflare_workers_ai", &entry.model),
                entry.input_modalities.iter().any(|value| value == "image")
            );
        }
        assert!(supports_images(
            "cloudflare_workers_ai",
            "@cf/moonshotai/kimi-k2.6"
        ));
        assert!(!supports_images(
            "cloudflare_workers_ai",
            "@cf/openai/gpt-oss-120b"
        ));
    }
}
