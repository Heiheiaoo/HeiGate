//! Model-name normalization. Ported from the Go `internal/channel/canonical.go`.
//!
//! **This has a deliberate frontend twin in `frontend/src/services/modelNaming.ts`.**
//! The UI groups model cards without a round-trip to the gateway, so a change
//! to the vendor-prefix list, a billing/date suffix, or an equivalence group
//! here must be mirrored there (and vice versa), or the UI will group models
//! differently from how the gateway routes them.

use std::collections::HashMap;
use std::sync::RwLock;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::store::Channel;

static CUSTOM_MAPPINGS: Lazy<RwLock<HashMap<String, String>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static VENDOR_PREFIX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(z-ai|qwen|deepseek-ai|deepseek|openai|anthropic|google|mistralai|mistral|meta-llama|meta|nvidia|01-ai|baichuan-inc|minimaxai|minimax|tencent|aliyun|moonshotai|bytedance|togethercomputer)/",
    )
    .unwrap()
});

static BILLING_SUFFIX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(:free|[-_]free|[-_]trial|[-_]vip|[-_]plus|[-_]preview|[-_]latest|[-_]next)$")
        .unwrap()
});

static DATE_SUFFIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(-\d{4}-\d{2}-\d{2}|-\d{8}|-\d{4})$").unwrap());

static PUNCTUATION: Lazy<Regex> = Lazy::new(|| Regex::new(r"[-_.\s/:]+").unwrap());

/// Updates the global in-memory custom model mappings. Keys are normalized to
/// lowercased trimmed strings.
pub fn set_custom_model_mappings(mappings: HashMap<String, String>) {
    let mut normalized = HashMap::with_capacity(mappings.len());
    for (key, value) in mappings {
        let key = key.trim().to_lowercase();
        let value = value.trim().to_string();
        if !key.is_empty() && !value.is_empty() {
            normalized.insert(key, value);
        }
    }
    *CUSTOM_MAPPINGS.write().unwrap() = normalized;
}

/// Installs the persisted `custom_model_mappings` setting (a JSON object of raw
/// name -> target). An absent, empty or malformed value leaves the table empty,
/// matching the Go loader, which simply skips a failed decode.
pub fn load_custom_model_mappings(raw: &str) {
    if raw.trim().is_empty() {
        return;
    }
    if let Ok(mappings) = serde_json::from_str::<HashMap<String, String>>(raw) {
        set_custom_model_mappings(mappings);
    }
}

/// Returns a copy of the current custom model mappings.
pub fn get_custom_model_mappings() -> HashMap<String, String> {
    CUSTOM_MAPPINGS.read().unwrap().clone()
}

/// Resolves a raw model name using custom mappings, checking a direct match, a
/// cleaned match (stripping namespace and `-free`), then a canonical key match.
pub fn resolve_custom_model_mapping(raw: &str) -> Option<String> {
    let raw_trim = raw.trim().to_lowercase();
    if raw_trim.is_empty() {
        return None;
    }
    let mappings = CUSTOM_MAPPINGS.read().unwrap();

    if let Some(target) = mappings.get(&raw_trim) {
        if !target.is_empty() {
            return Some(target.clone());
        }
    }

    let cleaned = clean_model_name(&raw_trim).to_lowercase();
    if !cleaned.is_empty() && cleaned != raw_trim {
        if let Some(target) = mappings.get(&cleaned) {
            if !target.is_empty() {
                return Some(target.clone());
            }
        }
    }

    let canon_key = canonical_model_key(&raw_trim);
    if !canon_key.is_empty() {
        for (key, value) in mappings.iter() {
            if canonical_model_key(key) == canon_key && !value.is_empty() {
                return Some(value.clone());
            }
        }
    }

    None
}

/// Computes a clean, lowercased alphanumeric key used to group aliases of the
/// same base model together across providers.
pub fn canonical_model_key(raw: &str) -> String {
    let cleaned = clean_model_name(raw);
    PUNCTUATION
        .replace_all(&cleaned.to_lowercase(), "")
        .into_owned()
}

/// Removes vendor namespaces, billing/policy suffixes, and date tags.
pub fn clean_model_name(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return String::new();
    }

    // 1. Remove a known vendor prefix, or a generic namespace unless the prefix
    //    looks like a model name.
    if VENDOR_PREFIX.is_match(&s) {
        s = VENDOR_PREFIX.replace(&s, "").into_owned();
    } else if let Some(idx) = s.find('/') {
        if idx < s.len() - 1 {
            let prefix = s[..idx].to_lowercase();
            if !prefix.contains("gpt") && !prefix.contains("claude") && !prefix.contains("gemini") {
                s = s[idx + 1..].to_string();
            }
        }
    }

    // 2. Remove a billing/policy suffix.
    s = BILLING_SUFFIX.replace(&s, "").into_owned();

    // 3. Remove a trailing date code.
    s = DATE_SUFFIX.replace(&s, "").into_owned();

    s.trim().to_string()
}

/// Equivalence groups mapping alternative keys to their canonical root. Kept
/// identical to the Go `knownEquivalenceGroups` table.
const EQUIVALENCE_GROUPS: &[&[&str]] = &[
    &["glm53", "glm53flash"],
    &["glm4", "glm4flash", "glm4air"],
    &["deepseekchat", "deepseekv3"],
    &["deepseekreasoner", "deepseekr1"],
    &["qwen38flash", "qwen38flashnext"],
    &["claude35sonnet", "claude35sonnet20241022"],
    &["claude35haiku", "claude35haiku20241022"],
    &["claude37sonnet", "claude37sonnet20250219"],
    &["claude3opus", "claude3opus20240229"],
    &["gpt4o", "gpt4o20240806", "gpt4o20240513"],
    &["gpt4omini", "gpt4omini20240718"],
];

/// Maps any model name or alias to its root canonical group identifier,
/// consulting user-defined custom mappings first (traversing up to 5 hops).
pub fn get_canonical_group_key(raw: &str) -> String {
    let raw_trim = raw.trim().to_string();
    if raw_trim.is_empty() {
        return String::new();
    }

    let mut curr = raw_trim.clone();
    for _ in 0..5 {
        match resolve_custom_model_mapping(&curr) {
            Some(target) if !target.is_empty() && !target.eq_ignore_ascii_case(&curr) => {
                curr = target;
            }
            _ => break,
        }
    }

    let key = canonical_model_key(&curr);
    if key.is_empty() {
        return curr;
    }
    for group in EQUIVALENCE_GROUPS {
        if group.contains(&key.as_str()) {
            return group[0].to_string();
        }
    }
    key
}

/// Whether two model names refer to the same underlying model.
pub fn are_models_equivalent(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    let key_a = get_canonical_group_key(a);
    let key_b = get_canonical_group_key(b);
    if key_a.is_empty() || key_b.is_empty() {
        return false;
    }
    key_a == key_b
}

/// If the channel can serve `requested_model`, returns the exact model string
/// configured on the channel and the match type (`exact`, `exact_ci`, `clean`,
/// `canonical`).
pub fn find_channel_model_for_request(
    channel: &Channel,
    requested_model: &str,
) -> Option<(String, &'static str)> {
    if requested_model.is_empty() {
        return None;
    }
    let req_trim = requested_model.trim();
    let mut all_models = vec![channel.primary_model.clone()];
    all_models.extend(channel.extra_models.clone());

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
    let req_clean = clean_model_name(req_trim);
    for m in &all_models {
        if clean_model_name(m).eq_ignore_ascii_case(&req_clean) {
            return Some((m.clone(), "clean"));
        }
    }
    for m in &all_models {
        if are_models_equivalent(m, req_trim) {
            return Some((m.clone(), "canonical"));
        }
    }
    None
}

/// Whether the channel has any model matching `requested_model`.
pub fn channel_supports_model_or_canonical(channel: &Channel, requested_model: &str) -> bool {
    find_channel_model_for_request(channel, requested_model).is_some()
}

/// Picks the cleanest, most standard display name from aliases discovered
/// across channels.
pub fn select_canonical_display_name(models: &[String]) -> String {
    if models.is_empty() {
        return String::new();
    }

    let custom_target = {
        let mappings = CUSTOM_MAPPINGS.read().unwrap();
        models.iter().find_map(|m| {
            mappings
                .get(&m.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .cloned()
        })
    };

    let mut candidate_list: Vec<String> = models.to_vec();
    if let Some(target) = &custom_target {
        let has_target = models.iter().any(|m| m.eq_ignore_ascii_case(target));
        if !has_target {
            candidate_list.push(target.clone());
        }
    }

    let mut best = String::new();
    let mut best_score: i32 = -100;

    for m in &candidate_list {
        let m = m.trim();
        if m.is_empty() {
            continue;
        }
        let mut score: i32 = 0;
        let lower = m.to_lowercase();

        if let Some(target) = &custom_target {
            if m.eq_ignore_ascii_case(target) {
                score += 60;
            }
        }
        if m.contains('/') {
            score -= 30;
        }
        if lower.contains("-free") || lower.contains(":free") || lower.contains("_free") {
            score -= 40;
        }
        if lower.contains("-trial") || lower.contains("-preview") || lower.contains("-next") {
            score -= 20;
        }
        if DATE_SUFFIX.is_match(&lower) {
            score -= 15;
        }
        if lower.contains("-flash") || lower.contains("-pro") || lower.contains("-turbo") {
            score += 10;
        }
        score -= m.len() as i32;

        if score > best_score || best.is_empty() {
            best_score = score;
            best = m.to_string();
        }
    }

    if !best.is_empty() {
        let cleaned = clean_model_name(&best);
        if !cleaned.is_empty() && !cleaned.contains('/') && !cleaned.to_lowercase().contains("free")
        {
            return cleaned;
        }
        return best;
    }
    models[0].clone()
}

#[cfg(test)]
mod tests {
    //! Ports `internal/channel/canonical_test.go`. The equivalence table and
    //! expected match results must stay identical to the Go originals.

    use super::*;

    #[test]
    fn canonical_model_key_groups_aliases() {
        assert_eq!(canonical_model_key("qwen3.8-flash"), "qwen38flash");
        assert_eq!(canonical_model_key("Qwen/Qwen3.8-Flash"), "qwen38flash");
        assert_eq!(canonical_model_key("glm-5.3-flash-free"), "glm53flash");
        assert_eq!(
            canonical_model_key("deepseek-v4-flash-0731"),
            "deepseekv4flash"
        );
        assert_eq!(clean_model_name("z-ai/glm-5.3-free"), "glm-5.3");
        assert_eq!(clean_model_name("  spaced  "), "spaced");
    }

    #[test]
    fn are_models_equivalent_table() {
        let cases: &[(&str, &str, bool)] = &[
            ("qwen3.8-flash", "qwen3.8-flash-free", true),
            ("qwen3.8-flash", "Qwen/Qwen3.8-Flash", true),
            ("qwen-3.8-flash", "qwen3.8-flash-free", true),
            ("qwen3.8-flash", "Qwen3.8-Flash-Next", true),
            ("qwen3.8-flash-free", "Qwen3.8-Flash-Next", true),
            ("glm-5.3-flash", "glm-5.3-flash-free", true),
            ("glm-5.3-flash", "z-ai/glm-5.3-free", true),
            ("glm-5.3-flash-free", "z-ai/glm-5.3-free", true),
            ("deepseek-v4-flash-0731", "deepseek-v4-flash", true),
            ("claude-3-5-sonnet-20241022", "claude-3-5-sonnet", true),
            ("gpt-4o-2024-08-06", "gpt-4o", true),
            ("mistral/codestral-2508", "codestral-2508", true),
            (
                "nvidia/llama-3.2-11b-vision-instruct",
                "meta-llama/llama-3.2-11b-vision-instruct",
                true,
            ),
            ("glm-5.3-flash", "qwen3.8-flash", false),
            ("gpt-4o", "gpt-4o-mini", false),
            ("qwen3.8-flash", "qwen3.8-27b", false),
        ];
        for (a, b, expected) in cases {
            assert_eq!(
                are_models_equivalent(a, b),
                *expected,
                "AreModelsEquivalent({a:?}, {b:?}) (keyA={}, keyB={})",
                canonical_model_key(a),
                canonical_model_key(b)
            );
        }
    }

    #[test]
    fn find_channel_model_matches_aliases() {
        let channel = Channel {
            id: 1,
            name: "Nexus".to_string(),
            primary_model: "glm-5.3-flash-free".to_string(),
            extra_models: vec![
                "qwen3.8-flash-free".to_string(),
                "deepseek-v4-flash-0731".to_string(),
            ],
            ..Default::default()
        };

        let (model, match_type) = find_channel_model_for_request(&channel, "glm-5.3-flash")
            .expect("glm-5.3-flash should match");
        assert_eq!(model, "glm-5.3-flash-free");
        assert_eq!(match_type, "clean");

        let (model, _) = find_channel_model_for_request(&channel, "qwen-3.8-flash")
            .expect("qwen-3.8-flash should match");
        assert_eq!(model, "qwen3.8-flash-free");

        let (model, _) = find_channel_model_for_request(&channel, "deepseek-v4-flash")
            .expect("deepseek-v4-flash should match");
        assert_eq!(model, "deepseek-v4-flash-0731");

        assert!(find_channel_model_for_request(&channel, "gpt-4o").is_none());
    }

    #[test]
    fn select_canonical_display_name_prefers_clean_names() {
        let glm = vec![
            "z-ai/glm-5.3-free".to_string(),
            "glm-5.3-flash-free".to_string(),
            "glm-5.3-flash".to_string(),
        ];
        assert_eq!(select_canonical_display_name(&glm), "glm-5.3-flash");

        let qwen = vec![
            "Qwen/Qwen3.8-Flash-Free".to_string(),
            "qwen3.8-flash-free".to_string(),
            "qwen3.8-flash".to_string(),
        ];
        assert_eq!(select_canonical_display_name(&qwen), "qwen3.8-flash");
    }

    #[test]
    fn custom_model_mappings() {
        let mut mappings = HashMap::new();
        mappings.insert(
            "deepseek-v4-flash-0731-free-3".to_string(),
            "deepseek-v4-flash".to_string(),
        );
        mappings.insert(
            "DeepSeek-V4-Flash-Vision-Exp".to_string(),
            "deepseek-v4-flash".to_string(),
        );
        set_custom_model_mappings(mappings);

        assert!(are_models_equivalent(
            "deepseek-v4-flash-0731-free-3",
            "deepseek-v4-flash"
        ));
        assert!(are_models_equivalent(
            "DeepSeek-V4-Flash-Vision-Exp",
            "deepseek-v4-flash"
        ));

        let channel = Channel {
            id: 2,
            name: "L站".to_string(),
            primary_model: "deepseek-v4-flash-0731-free-3".to_string(),
            ..Default::default()
        };
        let (model, _) = find_channel_model_for_request(&channel, "deepseek-v4-flash")
            .expect("custom mapping should match");
        assert_eq!(model, "deepseek-v4-flash-0731-free-3");

        let best = select_canonical_display_name(&["deepseek-v4-flash-0731-free-3".to_string()]);
        assert_eq!(best, "deepseek-v4-flash");

        // Restoring the persisted setting installs the same mappings. Kept in
        // this test because the mapping table is process-global and tests run
        // in parallel: only one test may own it.
        set_custom_model_mappings(HashMap::new());
        load_custom_model_mappings(r#"{"deepseek-flash":"DeepSeek-V4-Flash"}"#);
        assert_eq!(
            get_custom_model_mappings().get("deepseek-flash"),
            Some(&"DeepSeek-V4-Flash".to_string())
        );

        // A malformed or empty setting must leave the table empty rather than
        // panic, matching the Go loader.
        set_custom_model_mappings(HashMap::new());
        load_custom_model_mappings("not json");
        load_custom_model_mappings("");
        assert!(get_custom_model_mappings().is_empty());
    }
}
