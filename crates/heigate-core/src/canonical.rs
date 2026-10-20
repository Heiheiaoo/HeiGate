//! Model-ID matching. IDs are routing contracts and are not normalized into
//! aliases: only surrounding whitespace and case are ignored when matching.

use crate::store::Channel;

/// Computes a case-insensitive key while preserving the full model ID.
pub fn canonical_model_key(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// Trims display whitespace without changing the upstream model ID.
pub fn clean_model_name(raw: &str) -> String {
    raw.trim().to_string()
}

/// Returns a stable lookup key for an exact model ID. Model families,
/// provider prefixes and billing suffixes are deliberately not collapsed.
pub fn get_canonical_group_key(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// Whether two model names refer to the same underlying model.
pub fn are_models_equivalent(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim())
}

/// If the channel can serve `requested_model`, returns the exact model string
/// configured on the channel and the match type (`exact`, `exact_ci`).
pub fn find_channel_model_for_request(
    channel: &Channel,
    requested_model: &str,
) -> Option<(String, &'static str)> {
    if requested_model.is_empty() {
        return None;
    }
    let req_trim = requested_model.trim();
    let all_models = channel.configured_models();

    for m in &all_models {
        if m == req_trim {
            return Some((m.clone(), "exact"));
        }
    }
    for m in &all_models {
        if m.eq_ignore_ascii_case(req_trim) {
            return Some((m.clone(), "exact_ci"));
        }
    }
    None
}

/// Whether the channel has any model matching `requested_model`.
pub fn channel_supports_model(channel: &Channel, requested_model: &str) -> bool {
    find_channel_model_for_request(channel, requested_model).is_some()
}

/// Returns the first configured ID unchanged for display.
pub fn select_canonical_display_name(models: &[String]) -> String {
    models
        .iter()
        .find(|model| !model.trim().is_empty())
        .map(|model| model.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_normalization_preserves_the_full_id() {
        assert_eq!(
            canonical_model_key(" Qwen/Qwen3.8-Flash-Free "),
            "qwen/qwen3.8-flash-free"
        );
        assert_eq!(clean_model_name("z-ai/glm-5.3-free"), "z-ai/glm-5.3-free");
        assert_eq!(clean_model_name("  spaced  "), "spaced");
    }

    #[test]
    fn only_exact_ids_match_ignoring_case() {
        assert!(are_models_equivalent("GPT-4o", "gpt-4o"));
        assert!(!are_models_equivalent("gpt-4o", "gpt-4o-2024-08-06"));
        assert!(!are_models_equivalent("vendor/model", "model"));
        assert!(!are_models_equivalent("", ""));
    }

    #[test]
    fn request_matches_a_configured_id_without_aliasing() {
        let channel = Channel {
            id: 1,
            name: "Nexus".to_string(),
            primary_model: String::new(),
            extra_models: vec!["provider/model-v2:free".to_string()],
            ..Default::default()
        };

        let (model, match_type) =
            find_channel_model_for_request(&channel, "PROVIDER/MODEL-V2:FREE")
                .expect("case-insensitive exact ID should match");
        assert_eq!(model, "provider/model-v2:free");
        assert_eq!(match_type, "exact_ci");
        assert!(find_channel_model_for_request(&channel, "model-v2").is_none());
    }

    #[test]
    fn display_keeps_the_original_id() {
        let models = vec!["vendor/model-v2:free".to_string(), "model-v2".to_string()];
        assert_eq!(
            select_canonical_display_name(&models),
            "vendor/model-v2:free"
        );
    }
}
