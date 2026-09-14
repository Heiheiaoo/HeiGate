package probe

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
)

// answerFromPrompt extracts "a + b" from the challenge prompt and returns the sum.
func answerFromPrompt(t *testing.T, prompt string) string {
	t.Helper()
	m := regexp.MustCompile(`(\d+) \+ (\d+)`).FindStringSubmatch(prompt)
	require.Len(t, m, 3, "prompt should contain an addition: %q", prompt)
	a, _ := strconv.Atoi(m[1])
	b, _ := strconv.Atoi(m[2])
	return strconv.Itoa(a + b)
}

func TestNormalizeEndpoint(t *testing.T) {
	cases := map[string]string{
		"api.example.com":             "https://api.example.com",
		"https://api.example.com/":    "https://api.example.com",
		"https://api.example.com/v1":  "https://api.example.com",
		"https://api.example.com/v1/": "https://api.example.com",
		"http://127.0.0.1:11434/v1":   "http://127.0.0.1:11434",
		"  https://x.y/custom/base  ": "https://x.y/custom/base",
		"https://x.y/custom/base/v1/": "https://x.y/custom/base",
		"":                            "",
	}
	for in, want := range cases {
		require.Equal(t, want, NormalizeEndpoint(in), "input %q", in)
	}
}

func TestValidateConfig(t *testing.T) {
	ok := Config{Provider: ProviderOpenAI, Endpoint: "https://api.example.com", PrimaryModel: "gpt-x"}
	require.NoError(t, ValidateConfig(ok))

	bad := ok
	bad.Provider = "gemini"
	require.ErrorIs(t, ValidateConfig(bad), ErrInvalidProvider)

	bad = ok
	bad.Provider = ProviderAnthropic
	bad.APIMode = APIModeResponses
	require.ErrorIs(t, ValidateConfig(bad), ErrInvalidAPIMode)

	bad = ok
	bad.APIMode = "grpc"
	require.ErrorIs(t, ValidateConfig(bad), ErrInvalidAPIMode)

	bad = ok
	bad.Endpoint = "ftp://x"
	require.ErrorIs(t, ValidateConfig(bad), ErrInvalidEndpoint)

	bad = ok
	bad.Endpoint = ""
	require.ErrorIs(t, ValidateConfig(bad), ErrInvalidEndpoint)

	bad = ok
	bad.PrimaryModel = " "
	require.ErrorIs(t, ValidateConfig(bad), ErrMissingPrimaryModel)
}

func TestValidateInterval(t *testing.T) {
	require.NoError(t, ValidateInterval(0))
	require.NoError(t, ValidateInterval(15))
	require.NoError(t, ValidateInterval(86400))
	require.ErrorIs(t, ValidateInterval(5), ErrInvalidInterval)
	require.ErrorIs(t, ValidateInterval(86401), ErrInvalidInterval)
	require.ErrorIs(t, ValidateInterval(-1), ErrInvalidInterval)
}

func TestAnswerMatches(t *testing.T) {
	require.True(t, answerMatches("42", "42"))
	require.True(t, answerMatches("The answer is **42**.", "42"))
	require.True(t, answerMatches("042", "42"))
	require.False(t, answerMatches("420", "42"))
	require.False(t, answerMatches("", "42"))
	require.False(t, answerMatches("forty-two", "42"))
}

func TestRunOpenAIChatOperational(t *testing.T) {
	var gotAuth, gotPath string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		gotAuth = r.Header.Get("Authorization")
		gotPath = r.URL.Path
		var req struct {
			Model    string `json:"model"`
			Messages []struct {
				Role    string `json:"role"`
				Content string `json:"content"`
			} `json:"messages"`
		}
		require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
		require.Len(t, req.Messages, 1)
		answer := answerFromPrompt(t, req.Messages[0].Content)
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"choices":[{"message":{"role":"assistant","content":"`+answer+`"}}]}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{
		Provider: ProviderOpenAI, Endpoint: server.URL + "/v1", APIKey: "sk-test", PrimaryModel: "m1", ExtraModels: []string{"m1", "m2"},
	})
	require.NoError(t, err)
	require.Len(t, results, 2, "duplicate models must be collapsed")
	require.Equal(t, "Bearer sk-test", gotAuth)
	require.Equal(t, "/v1/chat/completions", gotPath)
	for _, res := range results {
		require.Equal(t, StatusOperational, res.Status, res.Message)
		require.NotNil(t, res.LatencyMs)
		require.NotNil(t, res.PingLatencyMs)
	}
}

func TestRunAnthropicOperational(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		require.Equal(t, "/v1/messages", r.URL.Path)
		require.Equal(t, "sk-ant", r.Header.Get("x-api-key"))
		require.Equal(t, "2023-06-01", r.Header.Get("anthropic-version"))
		var req struct {
			Messages []struct {
				Content string `json:"content"`
			} `json:"messages"`
		}
		require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
		answer := answerFromPrompt(t, req.Messages[0].Content)
		_, _ = io.WriteString(w, `{"content":[{"type":"text","text":"Sure! `+answer+`"}]}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderAnthropic, Endpoint: server.URL, APIKey: "sk-ant", PrimaryModel: "claude-x"})
	require.NoError(t, err)
	require.Len(t, results, 1)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
}

func TestRunResponsesOperational(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		require.Equal(t, "/v1/responses", r.URL.Path)
		var req struct {
			Input string `json:"input"`
		}
		require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
		answer := answerFromPrompt(t, req.Input)
		_, _ = io.WriteString(w, `{"output":[{"type":"message","content":[{"type":"output_text","text":"`+answer+`"}]}]}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, APIMode: APIModeResponses, Endpoint: server.URL, PrimaryModel: "gpt-x"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
}

func TestRunWrongAnswerIsFailed(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		_, _ = io.WriteString(w, `{"choices":[{"message":{"content":"I cannot do math."}}]}`)
	}))
	defer server.Close()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusFailed, results[0].Status)
	require.Contains(t, results[0].Message, "unexpected answer")
}

func TestRunHTTPErrorIsErrorAndRedacted(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		w.WriteHeader(http.StatusUnauthorized)
		_, _ = io.WriteString(w, `{"error":{"message":"bad key `+strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")+`"}}`)
	}))
	defer server.Close()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, APIKey: "sk-secret-value", PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusError, results[0].Status)
	require.Contains(t, results[0].Message, "upstream HTTP 401")
	require.NotContains(t, results[0].Message, "sk-secret-value")
}

func TestRunNetworkErrorIsError(t *testing.T) {
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: "http://127.0.0.1:1", PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusError, results[0].Status)
	require.NotEmpty(t, results[0].Message)
	require.Nil(t, results[0].PingLatencyMs)
}

func TestRunSlowAnswerIsDegraded(t *testing.T) {
	old := DegradedThreshold
	DegradedThreshold = 30 * time.Millisecond
	defer func() { DegradedThreshold = old }()

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		var req struct {
			Messages []struct {
				Content string `json:"content"`
			} `json:"messages"`
		}
		require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
		time.Sleep(60 * time.Millisecond)
		_, _ = io.WriteString(w, `{"choices":[{"message":{"content":"`+answerFromPrompt(t, req.Messages[0].Content)+`"}}]}`)
	}))
	defer server.Close()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusDegraded, results[0].Status)
	require.Contains(t, results[0].Message, "slow response")
}

func TestRunRejectsInvalidConfig(t *testing.T) {
	_, err := Run(context.Background(), Config{Provider: "nope", Endpoint: "https://x", PrimaryModel: "m"})
	require.ErrorIs(t, err, ErrInvalidProvider)
}

func TestExtractTextHandlesContentParts(t *testing.T) {
	cfg := Config{Provider: ProviderOpenAI, APIMode: APIModeChatCompletions}
	got := extractText(cfg, []byte(`{"choices":[{"message":{"content":[{"type":"text","text":"12"},{"type":"text","text":"3"}]}}]}`))
	require.Equal(t, "123", got)
	got = extractText(cfg, []byte(`{"choices":[{"message":{"content":"","reasoning_content":"thinking... 77"}}]}`))
	require.Equal(t, "thinking... 77", got)
}
