package service

import (
	"testing"
)

func TestCanonicalModelKeyAndEquivalence(t *testing.T) {
	tests := []struct {
		a        string
		b        string
		expected bool
	}{
		// User's exact prompt examples
		{"qwen3.8-flash", "qwen3.8-flash-free", true},
		{"qwen3.8-flash", "Qwen/Qwen3.8-Flash", true},
		{"qwen-3.8-flash", "qwen3.8-flash-free", true},
		{"qwen3.8-flash", "Qwen3.8-Flash-Next", true},
		{"qwen3.8-flash-free", "Qwen3.8-Flash-Next", true},
		{"glm-5.3-flash", "glm-5.3-flash-free", true},
		{"glm-5.3-flash", "z-ai/glm-5.3-free", true},
		{"glm-5.3-flash-free", "z-ai/glm-5.3-free", true},

		// Date micro-versions
		{"deepseek-v4-flash-0731", "deepseek-v4-flash", true},
		{"claude-3-5-sonnet-20241022", "claude-3-5-sonnet", true},
		{"gpt-4o-2024-08-06", "gpt-4o", true},

		// Namespaces
		{"mistral/codestral-2508", "codestral-2508", true},
		{"nvidia/llama-3.2-11b-vision-instruct", "meta-llama/llama-3.2-11b-vision-instruct", true},

		// Non-matching
		{"glm-5.3-flash", "qwen3.8-flash", false},
		{"gpt-4o", "gpt-4o-mini", false},
		{"qwen3.8-flash", "qwen3.8-27b", false},
	}

	for _, tt := range tests {
		got := AreModelsEquivalent(tt.a, tt.b)
		if got != tt.expected {
			t.Errorf("AreModelsEquivalent(%q, %q) = %v; want %v (keyA=%s, keyB=%s)",
				tt.a, tt.b, got, tt.expected, CanonicalModelKey(tt.a), CanonicalModelKey(tt.b))
		}
	}
}

func TestFindChannelModelForRequest(t *testing.T) {
	channel := &DesktopChannel{
		ID:           1,
		Name:         "Nexus",
		PrimaryModel: "glm-5.3-flash-free",
		ExtraModels:  []string{"qwen3.8-flash-free", "deepseek-v4-flash-0731"},
	}

	// 1. Client requests "glm-5.3-flash" -> Nexus should match and return "glm-5.3-flash-free"
	model, matchType, ok := FindChannelModelForRequest(channel, "glm-5.3-flash")
	if !ok || model != "glm-5.3-flash-free" {
		t.Fatalf("expected glm-5.3-flash-free, got %s (type=%s, ok=%v)", model, matchType, ok)
	}

	// 2. Client requests "qwen-3.8-flash" -> Nexus should match and return "qwen3.8-flash-free"
	model, matchType, ok = FindChannelModelForRequest(channel, "qwen-3.8-flash")
	if !ok || model != "qwen3.8-flash-free" {
		t.Fatalf("expected qwen3.8-flash-free, got %s (type=%s, ok=%v)", model, matchType, ok)
	}

	// 3. Client requests "deepseek-v4-flash" -> Nexus should match and return "deepseek-v4-flash-0731"
	model, matchType, ok = FindChannelModelForRequest(channel, "deepseek-v4-flash")
	if !ok || model != "deepseek-v4-flash-0731" {
		t.Fatalf("expected deepseek-v4-flash-0731, got %s (type=%s, ok=%v)", model, matchType, ok)
	}

	// 4. Client requests "gpt-4o" -> Nexus should not match
	_, _, ok = FindChannelModelForRequest(channel, "gpt-4o")
	if ok {
		t.Fatalf("expected no match for gpt-4o on Nexus")
	}
}

func TestSelectCanonicalDisplayName(t *testing.T) {
	candidates := []string{"z-ai/glm-5.3-free", "glm-5.3-flash-free", "glm-5.3-flash"}
	best := SelectCanonicalDisplayName(candidates)
	if best != "glm-5.3-flash" {
		t.Errorf("expected glm-5.3-flash, got %s", best)
	}

	qwenCandidates := []string{"Qwen/Qwen3.8-Flash-Free", "qwen3.8-flash-free", "qwen3.8-flash"}
	qwenBest := SelectCanonicalDisplayName(qwenCandidates)
	if qwenBest != "qwen3.8-flash" {
		t.Errorf("expected qwen3.8-flash, got %s", qwenBest)
	}
}

func TestCustomModelMappings(t *testing.T) {
	// Setup custom mapping: "deepseek-v4-flash-0731-free-3" -> "deepseek-v4-flash"
	SetCustomModelMappings(map[string]string{
		"deepseek-v4-flash-0731-free-3": "deepseek-v4-flash",
		"DeepSeek-V4-Flash-Vision-Exp":  "deepseek-v4-flash",
	})
	defer SetCustomModelMappings(nil)

	// 1. Equivalence check
	if !AreModelsEquivalent("deepseek-v4-flash-0731-free-3", "deepseek-v4-flash") {
		t.Errorf("expected deepseek-v4-flash-0731-free-3 to be equivalent to deepseek-v4-flash")
	}
	if !AreModelsEquivalent("DeepSeek-V4-Flash-Vision-Exp", "deepseek-v4-flash") {
		t.Errorf("expected DeepSeek-V4-Flash-Vision-Exp to be equivalent to deepseek-v4-flash")
	}

	// 2. Channel model matching
	channel := &DesktopChannel{
		ID:           2,
		Name:         "L站",
		PrimaryModel: "deepseek-v4-flash-0731-free-3",
	}
	matchedModel, matchType, ok := FindChannelModelForRequest(channel, "deepseek-v4-flash")
	if !ok || matchedModel != "deepseek-v4-flash-0731-free-3" {
		t.Fatalf("expected matched model deepseek-v4-flash-0731-free-3, got %s (type=%s, ok=%v)", matchedModel, matchType, ok)
	}

	// 3. Display name selection should pick the target
	candidates := []string{"deepseek-v4-flash-0731-free-3"}
	best := SelectCanonicalDisplayName(candidates)
	if best != "deepseek-v4-flash" {
		t.Errorf("expected canonical display name deepseek-v4-flash, got %s", best)
	}
}
