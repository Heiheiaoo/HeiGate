package probe

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/stretchr/testify/require"
)

// chatJSON renders a non-streaming chat response with the given usage.
func chatJSON(promptTokens int, content string) string {
	return fmt.Sprintf(`{"usage":{"prompt_tokens":%d},"choices":[{"message":{"content":%s}}]}`,
		promptTokens, mustJSON(content))
}

func TestCheckInjectionClean(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = fmt.Fprint(w, chatJSON(45, "Hello!"))
	}))
	defer server.Close()

	report := CheckInjection(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.Equal(t, InjectionClean, report.State)
	require.Equal(t, 45, report.PromptTokens)
	require.Empty(t, report.Content, "clean channels skip extraction")
}

func TestCheckInjectionDetectedExtractsContent(t *testing.T) {
	var n int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n++
		var req struct {
			Messages []struct {
				Content string `json:"content"`
			} `json:"messages"`
		}
		require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
		if n == 1 {
			_, _ = fmt.Fprint(w, chatJSON(1100, "Hello!"))
			return
		}
		// The extraction request.
		_, _ = fmt.Fprint(w, chatJSON(1100, "```\nYou are a lazy developer.\n```"))
	}))
	defer server.Close()

	report := CheckInjection(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.Equal(t, InjectionDetected, report.State)
	require.Equal(t, 1100, report.PromptTokens)
	require.Contains(t, report.Content, "lazy developer")
}

func TestCheckInjectionSuspectedButExtractionFails(t *testing.T) {
	var n int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n++
		if n == 1 {
			_, _ = fmt.Fprint(w, chatJSON(100, "Hello!"))
			return
		}
		w.WriteHeader(http.StatusBadGateway)
		_, _ = fmt.Fprint(w, `{"error":"boom"}`)
	}))
	defer server.Close()

	report := CheckInjection(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.Equal(t, InjectionSuspected, report.State)
	require.Equal(t, 100, report.PromptTokens)
	require.Contains(t, report.Content, "提取失败")
}

func TestCheckInjectionUpstreamError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		_, _ = fmt.Fprint(w, `{"error":"bad key"}`)
	}))
	defer server.Close()

	report := CheckInjection(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.Equal(t, InjectionError, report.State)
	require.NotEmpty(t, report.Message)
}

func TestCheckInjectionModelsDedupesAndReportsPerModel(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		_ = json.NewDecoder(r.Body).Decode(&req)
		// One model is injected, the other is clean.
		if req.Model == "dirty" {
			_, _ = fmt.Fprint(w, chatJSON(900, "Hello!"))
			return
		}
		_, _ = fmt.Fprint(w, chatJSON(45, "Hello!"))
	}))
	defer server.Close()

	reports := CheckInjectionModels(context.Background(),
		Config{Provider: ProviderOpenAI, Endpoint: server.URL},
		[]string{"clean", "dirty", "clean"}) // duplicate must collapse

	require.Len(t, reports, 2, "duplicate models must be deduplicated")
	require.Equal(t, "clean", reports[0].Model)
	require.Equal(t, InjectionClean, reports[0].State)
	require.Equal(t, "dirty", reports[1].Model)
	require.Equal(t, InjectionDetected, reports[1].State)
	require.Equal(t, 900, reports[1].PromptTokens)
}

// The primary-model wrapper and the batch call must agree on the verdict.
func TestCheckInjectionMatchesBatchResult(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = fmt.Fprint(w, chatJSON(300, "Hello!"))
	}))
	defer server.Close()

	cfg := Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"}
	single := CheckInjection(context.Background(), cfg)
	batch := CheckInjectionModels(context.Background(), cfg, []string{"m"})

	require.Len(t, batch, 1)
	require.Equal(t, single.State, batch[0].State)
	require.Equal(t, single.PromptTokens, batch[0].PromptTokens)
}
