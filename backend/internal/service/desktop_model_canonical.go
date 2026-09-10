package service

import (
	"regexp"
	"strings"
	"sync"
)

var (
	// Thread-safe custom model classification mappings (rawLower -> targetModel)
	customModelMappingsMu sync.RWMutex
	customModelMappings   = make(map[string]string)
)

// SetCustomModelMappings updates the global in-memory custom model mappings.
// Keys are normalized to lowercased trimmed strings.
func SetCustomModelMappings(mappings map[string]string) {
	customModelMappingsMu.Lock()
	defer customModelMappingsMu.Unlock()
	customModelMappings = make(map[string]string, len(mappings))
	for k, v := range mappings {
		kTrim := strings.TrimSpace(k)
		vTrim := strings.TrimSpace(v)
		if kTrim != "" && vTrim != "" {
			customModelMappings[strings.ToLower(kTrim)] = vTrim
		}
	}
}

// GetCustomModelMappings returns a copy of the current custom model mappings.
func GetCustomModelMappings() map[string]string {
	customModelMappingsMu.RLock()
	defer customModelMappingsMu.RUnlock()
	res := make(map[string]string, len(customModelMappings))
	for k, v := range customModelMappings {
		res[k] = v
	}
	return res
}

// ResolveCustomModelMapping resolves a raw model name using custom mappings, if present.
// It checks direct match, clean model match (stripping namespace and -free), and canonical key match.
func ResolveCustomModelMapping(raw string) (string, bool) {
	rawTrim := strings.ToLower(strings.TrimSpace(raw))
	if rawTrim == "" {
		return "", false
	}
	customModelMappingsMu.RLock()
	defer customModelMappingsMu.RUnlock()

	// 1. Direct match
	if target, ok := customModelMappings[rawTrim]; ok && target != "" {
		return target, true
	}

	// 2. Cleaned model match
	cleaned := strings.ToLower(CleanModelName(rawTrim))
	if cleaned != "" && cleaned != rawTrim {
		if target, ok := customModelMappings[cleaned]; ok && target != "" {
			return target, true
		}
	}

	// 3. Canonical key match
	canonKey := CanonicalModelKey(rawTrim)
	if canonKey != "" {
		for k, v := range customModelMappings {
			if CanonicalModelKey(k) == canonKey && v != "" {
				return v, true
			}
		}
	}

	return "", false
}

var (
	// Vendor namespace prefixes to strip (e.g. "z-ai/glm-5.3-free" -> "glm-5.3-free")
	vendorPrefixRegex = regexp.MustCompile(`^(?i)(z-ai|qwen|deepseek-ai|deepseek|openai|anthropic|google|mistralai|mistral|meta-llama|meta|nvidia|01-ai|baichuan-inc|minimaxai|minimax|tencent|aliyun|moonshotai|bytedance|togethercomputer)/`)

	// Common billing and deployment suffixes to strip
	// e.g. "-free", ":free", "_free", "-preview", "-latest", "-next", "-chat"
	billingSuffixRegex = regexp.MustCompile(`(?i)(:free|[-_]free|[-_]trial|[-_]vip|[-_]plus|[-_]preview|[-_]latest|[-_]next)$`)

	// Date suffixes like "-20241022", "-2024-08-06", "-0731"
	dateSuffixRegex = regexp.MustCompile(`(?i)(-\d{4}-\d{2}-\d{2}|-\d{8}|-\d{4})$`)

	// Punctuation cleaner for canonical key comparison
	punctuationCleaner = regexp.MustCompile(`[-_.\s/:]+`)
)

// CanonicalModelKey computes a clean, lowercased alphanumeric key used to group
// aliases of the same base model together across providers.
// Examples:
//
//	"qwen3.8-flash"        -> "qwen38flash"
//	"qwen3.8-flash-free"   -> "qwen38flash"
//	"qwen-3.8-flash"       -> "qwen38flash"
//	"Qwen/Qwen3.8-Flash"   -> "qwen38flash"
//	"Qwen3.8-Flash-Next"   -> "qwen38flash"
//	"glm-5.3-flash"        -> "glm53flash"
//	"glm-5.3-flash-free"   -> "glm53flash"
//	"z-ai/glm-5.3-free"    -> "glm53" (or matches glm53flash via alias map)
//	"deepseek-v4-flash-0731" -> "deepseekv4flash"
func CanonicalModelKey(raw string) string {
	cleaned := CleanModelName(raw)
	return punctuationCleaner.ReplaceAllString(strings.ToLower(cleaned), "")
}

// CleanModelName removes vendor namespaces, billing/policy suffixes, and date tags.
func CleanModelName(raw string) string {
	s := strings.TrimSpace(raw)
	if s == "" {
		return ""
	}

	// 1. Remove vendor prefix
	if vendorPrefixRegex.MatchString(s) {
		s = vendorPrefixRegex.ReplaceAllString(s, "")
	} else if idx := strings.Index(s, "/"); idx != -1 && idx < len(s)-1 {
		// Generic namespace strip if prefix doesn't look like a model name
		prefix := strings.ToLower(s[:idx])
		if !strings.Contains(prefix, "gpt") && !strings.Contains(prefix, "claude") && !strings.Contains(prefix, "gemini") {
			s = s[idx+1:]
		}
	}

	// 2. Remove OpenRouter ":free" or similar
	s = billingSuffixRegex.ReplaceAllString(s, "")

	// 3. Remove trailing date code if present
	s = dateSuffixRegex.ReplaceAllString(s, "")

	return strings.TrimSpace(s)
}

// KnownEquivalencePairs maps alternative keys to their canonical equivalents.
// For example, in many free aggregators:
//
//	"glm53" <=> "glm53flash" (e.g. z-ai/glm-5.3-free vs glm-5.3-flash)
//	"deepseekchat" <=> "deepseekv3"
//	"deepseekreasoner" <=> "deepseekr1"
//	"qwen38flash" <=> "qwen38flashnext"
var knownEquivalenceGroups = [][]string{
	{"glm53", "glm53flash"},
	{"glm4", "glm4flash", "glm4air"},
	{"deepseekchat", "deepseekv3"},
	{"deepseekreasoner", "deepseekr1"},
	{"qwen38flash", "qwen38flashnext"},
	{"claude35sonnet", "claude35sonnet20241022"},
	{"claude35haiku", "claude35haiku20241022"},
	{"claude37sonnet", "claude37sonnet20250219"},
	{"claude3opus", "claude3opus20240229"},
	{"gpt4o", "gpt4o20240806", "gpt4o20240513"},
	{"gpt4omini", "gpt4omini20240718"},
}

// GetCanonicalGroupKey maps any model name or alias to its root canonical group identifier.
// For example, "glm-5.3-flash", "glm-5.3", "z-ai/glm-5.3-free" all map to "glm53".
// "Qwen3.8-Flash-Next", "qwen3.8-flash-free", "qwen3.8-flash" all map to "qwen38flash".
// It first consults any user-defined custom model classification mappings.
func GetCanonicalGroupKey(raw string) string {
	rawTrim := strings.TrimSpace(raw)
	if rawTrim == "" {
		return ""
	}
	// Check custom mappings first (traverse up to 5 hops to resolve chained mappings)
	curr := rawTrim
	for i := 0; i < 5; i++ {
		if target, ok := ResolveCustomModelMapping(curr); ok && target != "" && !strings.EqualFold(target, curr) {
			curr = target
		} else {
			break
		}
	}

	key := CanonicalModelKey(curr)
	if key == "" {
		return curr
	}
	for _, group := range knownEquivalenceGroups {
		for _, member := range group {
			if member == key {
				return group[0]
			}
		}
	}
	return key
}

// AreModelsEquivalent checks if two model names refer to the same underlying model.
func AreModelsEquivalent(a, b string) bool {
	if strings.EqualFold(a, b) {
		return true
	}
	keyA := GetCanonicalGroupKey(a)
	keyB := GetCanonicalGroupKey(b)
	if keyA == "" || keyB == "" {
		return false
	}
	return keyA == keyB
}

// FindChannelModelForRequest determines if a channel can serve requestedModel.
// If matched, it returns the exact model string configured on that channel (upstreamModel),
// the match type ("exact", "clean", or "canonical"), and true.
func FindChannelModelForRequest(channel *DesktopChannel, requestedModel string) (string, string, bool) {
	if channel == nil || requestedModel == "" {
		return "", "", false
	}
	reqTrim := strings.TrimSpace(requestedModel)
	allModels := append([]string{channel.PrimaryModel}, channel.ExtraModels...)

	// Pass 1: Exact match (case-sensitive)
	for _, m := range allModels {
		if m == reqTrim {
			return m, "exact", true
		}
	}

	// Pass 2: Case-insensitive exact match
	for _, m := range allModels {
		if strings.EqualFold(m, reqTrim) {
			return m, "exact_ci", true
		}
	}

	// Pass 3: Clean model name match
	reqClean := CleanModelName(reqTrim)
	for _, m := range allModels {
		if strings.EqualFold(CleanModelName(m), reqClean) {
			return m, "clean", true
		}
	}

	// Pass 4: Canonical key & equivalence group match
	for _, m := range allModels {
		if AreModelsEquivalent(m, reqTrim) {
			return m, "canonical", true
		}
	}

	return "", "", false
}

// ChannelSupportsModelOrCanonical returns true if the channel has any model matching requestedModel.
func ChannelSupportsModelOrCanonical(channel *DesktopChannel, requestedModel string) bool {
	_, _, ok := FindChannelModelForRequest(channel, requestedModel)
	return ok
}

// SelectCanonicalDisplayName picks the cleanest, most standard display name
// from a list of model aliases discovered across channels.
// Example input: ["z-ai/glm-5.3-free", "glm-5.3-flash-free", "glm-5.3-flash"]
// Returns: "glm-5.3-flash"
func SelectCanonicalDisplayName(models []string) string {
	if len(models) == 0 {
		return ""
	}

	// If a custom mapping target exists for any of the input models, include it as a prioritized candidate
	var customTarget string
	customModelMappingsMu.RLock()
	for _, m := range models {
		if t, ok := customModelMappings[strings.ToLower(strings.TrimSpace(m))]; ok && t != "" {
			customTarget = t
			break
		}
	}
	customModelMappingsMu.RUnlock()

	candidateList := models
	if customTarget != "" {
		hasTarget := false
		for _, m := range models {
			if strings.EqualFold(m, customTarget) {
				hasTarget = true
				break
			}
		}
		if !hasTarget {
			candidateList = append(append([]string{}, models...), customTarget)
		}
	}

	var best string
	bestScore := -100

	for _, m := range candidateList {
		m = strings.TrimSpace(m)
		if m == "" {
			continue
		}
		score := 0
		lower := strings.ToLower(m)

		// High priority bonus for user-specified custom target
		if customTarget != "" && strings.EqualFold(m, customTarget) {
			score += 60
		}

		// Penalize vendor namespace prefixes
		if strings.Contains(m, "/") {
			score -= 30
		}
		// Penalize billing suffixes
		if strings.Contains(lower, "-free") || strings.Contains(lower, ":free") || strings.Contains(lower, "_free") {
			score -= 40
		}
		if strings.Contains(lower, "-trial") || strings.Contains(lower, "-preview") || strings.Contains(lower, "-next") {
			score -= 20
		}
		// Penalize trailing date codes
		if dateSuffixRegex.MatchString(lower) {
			score -= 15
		}

		// Bonus for clean, standard casing and hyphenation
		if strings.Contains(lower, "-flash") || strings.Contains(lower, "-pro") || strings.Contains(lower, "-turbo") {
			score += 10
		}
		// Shorter cleaner names usually preferred
		score -= len(m)

		if score > bestScore || best == "" {
			bestScore = score
			best = m
		}
	}

	if best != "" {
		// If best still has vendor prefix or -free (e.g. only "z-ai/glm-5.3-free" existed)
		cleaned := CleanModelName(best)
		if cleaned != "" && !strings.Contains(cleaned, "/") && !strings.Contains(strings.ToLower(cleaned), "free") {
			return cleaned
		}
		return best
	}
	return models[0]
}
