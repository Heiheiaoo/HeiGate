package probe

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
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

// writeSSE emits server-sent-event frames and the OpenAI [DONE] sentinel.
func writeSSE(w http.ResponseWriter, events ...string) {
	w.Header().Set("Content-Type", "text/event-stream")
	for _, e := range events {
		_, _ = io.WriteString(w, "data: "+e+"\n\n")
	}
	_, _ = io.WriteString(w, "data: [DONE]\n\n")
}

func chatDelta(content string) string {
	return fmt.Sprintf(`{"choices":[{"delta":{"content":%s}}]}`, mustJSON(content))
}

func chatReasoningDelta(thinking string) string {
	return fmt.Sprintf(`{"choices":[{"delta":{"reasoning_content":%s}}]}`, mustJSON(thinking))
}

func anthropicTextDelta(text string) string {
	return fmt.Sprintf(`{"type":"content_block_delta","delta":{"type":"text_delta","text":%s}}`, mustJSON(text))
}

func anthropicThinkingDelta(thinking string) string {
	return fmt.Sprintf(`{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":%s}}`, mustJSON(thinking))
}

func responsesTextDelta(text string) string {
	return fmt.Sprintf(`{"type":"response.output_text.delta","delta":%s}`, mustJSON(text))
}

func mustJSON(s string) string {
	b, err := json.Marshal(s)
	if err != nil {
		panic(err)
	}
	return string(b)
}

func chatPrompt(t *testing.T, r *http.Request) string {
	t.Helper()
	var req struct {
		Messages []struct {
			Content string `json:"content"`
		} `json:"messages"`
	}
	require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
	require.NotEmpty(t, req.Messages)
	return req.Messages[0].Content
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
	// Two models are probed concurrently, so the handler runs on two goroutines.
	var mu sync.Mutex
	var gotAuth, gotPath string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		mu.Lock()
		gotAuth = r.Header.Get("Authorization")
		gotPath = r.URL.Path
		mu.Unlock()
		answer := answerFromPrompt(t, chatPrompt(t, r))
		writeSSE(w, chatDelta(answer))
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
		require.Nil(t, res.PingLatencyMs, "probe no longer performs a separate HEAD ping")
		require.NotNil(t, res.TTFTMs, "streaming probe must measure time to first token")
		require.Equal(t, *res.TTFTMs, *res.LatencyMs, "probe latency is intentionally the first-token time")
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
		answer := answerFromPrompt(t, chatPrompt(t, r))
		writeSSE(w, anthropicTextDelta("Sure! "+answer))
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderAnthropic, Endpoint: server.URL, APIKey: "sk-ant", PrimaryModel: "claude-x"})
	require.NoError(t, err)
	require.Len(t, results, 1)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
	require.NotNil(t, results[0].TTFTMs)
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
		writeSSE(w, responsesTextDelta(answer))
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, APIMode: APIModeResponses, Endpoint: server.URL, PrimaryModel: "gpt-x"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
	require.NotNil(t, results[0].TTFTMs)
}

// TTFT must be the first visible text token, not the first reasoning token:
// reasoning streams out immediately but the client still sees nothing.
func TestRunTTFTSkipsReasoningDeltas(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		answer := answerFromPrompt(t, chatPrompt(t, r))
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+chatReasoningDelta("thinking about it...")+"\n\n")
		time.Sleep(120 * time.Millisecond)
		_, _ = io.WriteString(w, "data: "+chatDelta(answer)+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
	require.NotNil(t, results[0].TTFTMs)
	require.GreaterOrEqual(t, *results[0].TTFTMs, 100, "TTFT should skip the reasoning phase")
}

// A model that answers entirely inside thinking has no text token; the TTFT
// falls back to the first reasoning delta so the metric is still comparable.
func TestRunTTFTFallsBackToReasoningOnly(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		answer := answerFromPrompt(t, chatPrompt(t, r))
		writeSSE(w, chatReasoningDelta("The answer is "+answer))
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, "reasoning content must still be scanned for the answer")
	require.NotNil(t, results[0].TTFTMs, "reasoning-only streams still report a TTFT")
}

func TestRunAnthropicThinkingFallback(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		answer := answerFromPrompt(t, chatPrompt(t, r))
		writeSSE(w, anthropicThinkingDelta("Answer: "+answer))
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderAnthropic, Endpoint: server.URL, APIKey: "sk-ant", PrimaryModel: "claude-x"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
	require.NotNil(t, results[0].TTFTMs)
}

// The probe only needs generated content; it must not reject a valid stream
// because the first delta contains only a partial answer.
func TestRunGeneratedContentStaysOperational(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		writeSSE(w, chatDelta("I cannot do math."))
	}))
	defer server.Close()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status)
	require.Empty(t, results[0].Message)
}

// A 2xx that streams nothing at all is the failure worth catching.
func TestRunEmptyStreamIsFailed(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		writeSSE(w, `{"choices":[{"delta":{}}]}`)
	}))
	defer server.Close()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusFailed, results[0].Status)
	require.Contains(t, results[0].Message, "no content")
}

// The probe reads the short capped stream so later stream errors are visible.
func TestRunReadsCompletedContentStream(t *testing.T) {
	var wroteAll bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		flusher, _ := w.(http.Flusher)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+chatDelta("8")+"\n\n")
		if flusher != nil {
			flusher.Flush()
		}
		// The capped probe should consume the complete short stream.
		time.Sleep(300 * time.Millisecond)
		for i := 0; i < 50; i++ {
			if _, err := io.WriteString(w, "data: "+chatDelta("more")+"\n\n"); err != nil {
				return
			}
			if flusher != nil {
				flusher.Flush()
			}
		}
		wroteAll = true
	}))
	defer server.Close()

	start := time.Now()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	elapsed := time.Since(start)
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
	require.GreaterOrEqual(t, elapsed, 300*time.Millisecond, "probe should wait for the completed capped stream")
	require.True(t, wroteAll, "handler should finish writing the capped stream")
}

func TestRunReadsPastReasoningDelta(t *testing.T) {
	var wroteText atomic.Bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		flusher, _ := w.(http.Flusher)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, `data: {"choices":[{"delta":{"reasoning":"thinking"}}]}`+"\n\n")
		if flusher != nil {
			flusher.Flush()
		}
		time.Sleep(300 * time.Millisecond)
		_, _ = io.WriteString(w, `data: {"choices":[{"delta":{"content":"later"}}]}`+"\n\n")
		wroteText.Store(true)
	}))
	defer server.Close()

	start := time.Now()
	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status)
	require.GreaterOrEqual(t, time.Since(start), 300*time.Millisecond)
	require.True(t, wroteText.Load())
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
		answer := answerFromPrompt(t, chatPrompt(t, r))
		time.Sleep(60 * time.Millisecond)
		writeSSE(w, chatDelta(answer))
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

func TestContentToTextHandlesParts(t *testing.T) {
	got := contentToText([]byte(`[{"type":"text","text":"12"},{"type":"text","text":"3"}]`))
	require.Equal(t, "123", got)
	got = contentToText([]byte(`"plain"`))
	require.Equal(t, "plain", got)
	require.Equal(t, "", contentToText(nil))
}

func TestRunDoesNotRetryEmptyAnswer(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		calls++
		writeSSE(w, `{"choices":[{"delta":{}}]}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusFailed, results[0].Status)
	require.Equal(t, 1, calls)
}

func TestRunDoesNotRetryServerError(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		calls++
		w.WriteHeader(http.StatusBadGateway)
		_, _ = io.WriteString(w, `{"error":"do request failed"}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusError, results[0].Status)
	require.Equal(t, 1, calls)
}

// A 4xx is recorded immediately; every probe consists of one attempt.
func TestRunDoesNotRetryClientError(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		calls++
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"error":"model not found"}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusError, results[0].Status)
	require.Equal(t, 1, calls, "4xx should not be retried")
}

func TestRunStopsAfterOneAttempt(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		calls++
		writeSSE(w, `{"choices":[{"delta":{}}]}`)
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusFailed, results[0].Status)
	require.Equal(t, 1, calls)
}

func TestRunBudgetFitsOneAttempt(t *testing.T) {
	require.GreaterOrEqual(t, RunBudget(), RequestTimeout,
		"RunBudget must cover the single attempt")
}

func TestRunUsesOneAttempt(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		calls++
		time.Sleep(200 * time.Millisecond)
		writeSSE(w, `{"choices":[{"delta":{}}]}`)
	}))
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), RunBudget())
	defer cancel()

	results, err := Run(ctx, Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, 1, calls)
	require.Equal(t, StatusFailed, results[0].Status)
}

func TestRunOnceDoesNotRetry(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		calls++
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"choices\":[{\"delta\":{}}]}\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	results, err := RunOnce(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Len(t, results, 1)
	require.Equal(t, 1, calls)
	require.Equal(t, StatusFailed, results[0].Status)
}

// Some OpenAI-compatible upstreams (PM among them) emit the thinking deltas
// under "reasoning" instead of the common "reasoning_content"; the probe must
// still see them, both for answer matching and the TTFT fallback.
func TestRunReadsNonstandardReasoningField(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		answer := answerFromPrompt(t, chatPrompt(t, r))
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":"","reasoning":"The sum is %s."}}]}`, answer)+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusOperational, results[0].Status, results[0].Message)
	require.NotNil(t, results[0].TTFTMs, "reasoning-only streams still report a TTFT")
}

func TestRunExtractsInStreamError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "event: error\n")
		_, _ = io.WriteString(w, `data: {"error":{"message":"503 Service Unavailable: No available workers"}}`+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	results, err := Run(context.Background(), Config{Provider: ProviderOpenAI, Endpoint: server.URL, PrimaryModel: "m"})
	require.NoError(t, err)
	require.Equal(t, StatusError, results[0].Status)
	require.Contains(t, results[0].Message, "503 Service Unavailable: No available workers")
}
