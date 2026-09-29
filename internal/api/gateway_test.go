package api

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/Heiheiaoo/heigate/internal/channel"
	"github.com/Heiheiaoo/heigate/internal/probe"
	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
	_ "modernc.org/sqlite"
)

func TestForwardDesktopRequestInjectsAnthropicKeyAndPath(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, "/v1/messages", r.URL.Path)
		require.Equal(t, "channel-secret", r.Header.Get("x-api-key"))
		require.Empty(t, r.Header.Get("Authorization"))
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: hello\n\n")
	}))
	defer server.Close()

	inbound := httptest.NewRequest(http.MethodPost, "/v1/messages", strings.NewReader(`{"model":"claude-test","messages":[]}`))
	response, err := forwardUpstream(context.Background(), inbound, []byte(`{"model":"claude-test","messages":[]}`), &channel.Channel{
		Provider: probe.ProviderAnthropic,
		Endpoint: server.URL,
		APIKey:   "channel-secret",
	}, protocolAnthropic, "claude-test")
	require.NoError(t, err)
	require.Equal(t, http.StatusOK, response.status)
	body, err := io.ReadAll(response.body)
	require.NoError(t, err)
	require.Equal(t, "data: hello\n\n", string(body))
	require.NoError(t, response.body.Close())
}

func TestForwardDesktopRequestTranslatesAnthropicToOpenAI(t *testing.T) {
	// Upstream is an OpenAI endpoint that accepts /v1/chat/completions and returns OpenAI JSON
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, "/v1/chat/completions", r.URL.Path)
		require.Equal(t, "Bearer openai-secret", r.Header.Get("Authorization"))
		var req struct {
			Model    string `json:"model"`
			Messages []struct {
				Role    string `json:"role"`
				Content string `json:"content"`
			} `json:"messages"`
		}
		require.NoError(t, json.NewDecoder(r.Body).Decode(&req))
		require.Equal(t, "deepseek-v4-flash", req.Model)
		require.Len(t, req.Messages, 1)
		require.Equal(t, "user", req.Messages[0].Role)
		require.Equal(t, "hello", req.Messages[0].Content)

		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"id":"chatcmpl-123","object":"chat.completion","created":1700000000,"model":"deepseek-v4-flash","choices":[{"index":0,"message":{"role":"assistant","content":"world"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}}`)
	}))
	defer server.Close()

	anthropicPayload := `{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hello"}]}`
	inbound := httptest.NewRequest(http.MethodPost, "/v1/messages", strings.NewReader(anthropicPayload))
	response, err := forwardUpstream(context.Background(), inbound, []byte(anthropicPayload), &channel.Channel{
		Provider:     probe.ProviderOpenAI,
		APIMode:      probe.APIModeChatCompletions,
		Endpoint:     server.URL,
		APIKey:       "openai-secret",
		PrimaryModel: "deepseek-v4-flash",
	}, protocolAnthropic, "deepseek-v4-flash")

	require.NoError(t, err)
	require.Equal(t, http.StatusOK, response.status)
	body, err := io.ReadAll(response.body)
	require.NoError(t, err)

	// Response must be formatted as Anthropic message JSON
	var anthropicResp struct {
		ID      string `json:"id"`
		Type    string `json:"type"`
		Role    string `json:"role"`
		Content []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"content"`
		StopReason string `json:"stop_reason"`
	}
	require.NoError(t, json.Unmarshal(body, &anthropicResp))
	require.Equal(t, "assistant", anthropicResp.Role)
	require.Len(t, anthropicResp.Content, 1)
	require.Equal(t, "world", anthropicResp.Content[0].Text)
	require.Equal(t, "end_turn", anthropicResp.StopReason)
}

func TestForwardDesktopRequestTranslatesAnthropicStreamingToOpenAISSE(t *testing.T) {
	// Upstream is an OpenAI endpoint that streams chat chunks
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, "/v1/chat/completions", r.URL.Path)
		w.Header().Set("Content-Type", "text/event-stream")
		flusher, ok := w.(http.Flusher)
		if !ok {
			http.Error(w, "expected streaming response writer", http.StatusInternalServerError)
			return
		}
		_, _ = io.WriteString(w, "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hi\"},\"finish_reason\":null}]}\n\n")
		flusher.Flush()
		_, _ = io.WriteString(w, "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" there\"},\"finish_reason\":\"stop\"}]}\n\n")
		flusher.Flush()
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
		flusher.Flush()
	}))
	defer server.Close()

	anthropicPayload := `{"model":"claude-sonnet-4-6","stream":true,"messages":[{"role":"user","content":"hello"}]}`
	inbound := httptest.NewRequest(http.MethodPost, "/v1/messages", strings.NewReader(anthropicPayload))
	response, err := forwardUpstream(context.Background(), inbound, []byte(anthropicPayload), &channel.Channel{
		Provider:     probe.ProviderOpenAI,
		APIMode:      probe.APIModeChatCompletions,
		Endpoint:     server.URL,
		APIKey:       "openai-secret",
		PrimaryModel: "deepseek-v4-flash",
	}, protocolAnthropic, "deepseek-v4-flash")

	require.NoError(t, err)
	require.Equal(t, http.StatusOK, response.status)
	body, err := io.ReadAll(response.body)
	require.NoError(t, err)
	sseText := string(body)

	// Must contain Anthropic standard events
	require.Contains(t, sseText, "event: message_start")
	require.Contains(t, sseText, "event: content_block_start")
	require.Contains(t, sseText, "event: content_block_delta")
	require.Contains(t, sseText, "event: message_delta")
	require.Contains(t, sseText, "event: message_stop")
}

func TestForwardTranslatedStreamingErrorPayloadIsNotSuccessful(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"error\":{\"message\":\"rate limit\"}}\n\n")
	}))
	defer server.Close()

	payload := `{"model":"claude-test","stream":true,"messages":[{"role":"user","content":"hello"}]}`
	response, err := forwardUpstream(context.Background(), httptest.NewRequest(http.MethodPost, "/v1/messages", strings.NewReader(payload)), []byte(payload), &channel.Channel{
		Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions, Endpoint: server.URL, APIKey: "key", PrimaryModel: "m",
	}, protocolAnthropic, "m")
	require.NoError(t, err)
	_, readErr := io.ReadAll(response.body)
	require.Error(t, readErr)
	require.Contains(t, readErr.Error(), "rate limit")
}

func TestForwardTranslatedStreamingWithoutCompletionMarkerFails(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n")
	}))
	defer server.Close()

	payload := `{"model":"claude-test","stream":true,"messages":[{"role":"user","content":"hello"}]}`
	response, err := forwardUpstream(context.Background(), httptest.NewRequest(http.MethodPost, "/v1/messages", strings.NewReader(payload)), []byte(payload), &channel.Channel{
		Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions, Endpoint: server.URL, APIKey: "key", PrimaryModel: "m",
	}, protocolAnthropic, "m")
	require.NoError(t, err)
	body, readErr := io.ReadAll(response.body)
	require.Error(t, readErr)
	require.Contains(t, string(body), "partial")
	require.NotContains(t, string(body), "message_stop")
}

func TestForwardDesktopRequestRejectsEmptySuccessfulResponse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	store := newTestStoreForHandler(t)
	ch := &channel.Channel{
		Name: "empty", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions,
		Endpoint: server.URL, APIKey: "key", PrimaryModel: "m", Enabled: true, LastStatus: probe.StatusOperational,
	}
	require.NoError(t, store.Save(context.Background(), ch))
	h := NewHandler(store, channel.NewProbeRunner(store))
	r := gin.New()
	r.POST("/v1/chat/completions", h.forwardChatCompletions)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/v1/chat/completions", strings.NewReader(`{"model":"m","messages":[]}`))
	r.ServeHTTP(rec, req)
	require.Equal(t, http.StatusBadGateway, rec.Code)
}

func TestGatewayModelsReturnsEnabledUniqueModels(t *testing.T) {
	store, err := channel.OpenStore(":memory:")
	require.NoError(t, err)
	defer func() { _ = store.Close() }()
	ch := &channel.Channel{
		Name: "local", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeResponses,
		Endpoint: "https://example.com", APIKey: "secret", PrimaryModel: "gpt-test", ExtraModels: []string{"gpt-test", "claude-test"}, Enabled: true,
	}
	require.NoError(t, store.Save(context.Background(), ch))
	h := NewHandler(store, channel.NewProbeRunner(store))
	gin.SetMode(gin.TestMode)
	r := gin.New()
	r.GET("/v1/models", h.gatewayModels)
	recorder := httptest.NewRecorder()
	r.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/v1/models", nil))
	require.Equal(t, http.StatusOK, recorder.Code)
	require.Contains(t, recorder.Body.String(), `"gpt-test"`)
	require.Contains(t, recorder.Body.String(), `"claude-test"`)
	require.Contains(t, recorder.Body.String(), `"Auto"`)
}

func TestBestAutoModelPrefersHealthyFastModel(t *testing.T) {
	primaryLatency, extraTTFT := 9000, 800
	ch := &channel.Channel{
		PrimaryModel:  "slow-primary",
		ExtraModels:   []string{"fast-extra", "unknown"},
		LastStatus:    probe.StatusDegraded,
		LastLatencyMs: &primaryLatency,
		ModelHealth: []channel.ModelHealth{
			{Model: "fast-extra", Status: probe.StatusOperational, TTFTMs: &extraTTFT},
			{Model: "unknown", Status: "unknown"},
		},
	}
	require.Equal(t, "fast-extra", bestAutoModel(ch))
}

func TestStreamTokenTrackerOpenAIUsage(t *testing.T) {
	tracker := &streamTokenTracker{promptTokens: 10}
	chunk1 := []byte("data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n")
	tracker.Feed(chunk1)
	require.Greater(t, tracker.completionTokens, 0)
	require.False(t, tracker.hasExactTokens)

	// Stream final chunk with exact usage
	chunk2 := []byte("data: {\"id\":\"chatcmpl-1\",\"choices\":[],\"usage\":{\"prompt_tokens\":15,\"completion_tokens\":42,\"total_tokens\":57}}\n\ndata: [DONE]\n\n")
	tracker.Feed(chunk2)
	require.True(t, tracker.hasExactTokens)
	require.Equal(t, 15, tracker.promptTokens)
	require.Equal(t, 42, tracker.completionTokens)
	require.Equal(t, 57, tracker.totalTokens)
}

func TestStreamTokenTrackerAnthropicUsage(t *testing.T) {
	tracker := &streamTokenTracker{}
	startChunk := []byte("data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":50}}}\n\n")
	tracker.Feed(startChunk)
	require.Equal(t, 50, tracker.promptTokens)

	deltaChunk := []byte("data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"世界你好，这是一段测试\"}}\n\n")
	tracker.Feed(deltaChunk)
	require.Greater(t, tracker.completionTokens, 0)

	stopChunk := []byte("data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":28}}\n\n")
	tracker.Feed(stopChunk)
	require.True(t, tracker.hasExactTokens)
	require.Equal(t, 50, tracker.promptTokens)
	require.Equal(t, 28, tracker.completionTokens)
}

func TestStreamTokenTrackerSawDone(t *testing.T) {
	// Case 1: OpenAI data: [DONE]
	tracker1 := &streamTokenTracker{}
	tracker1.Feed([]byte("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"))
	require.True(t, tracker1.sawDone)
	require.Equal(t, "stop", tracker1.finishReason)

	// Case 2: Anthropic message_stop
	tracker2 := &streamTokenTracker{}
	tracker2.Feed([]byte("data: {\"type\":\"message_stop\"}\n\n"))
	require.True(t, tracker2.sawDone)
	require.Equal(t, "stop", tracker2.finishReason)

	// OpenAI Responses API uses response.completed instead of [DONE].
	tracker3 := &streamTokenTracker{}
	tracker3.Feed([]byte("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"))
	require.True(t, tracker3.sawDone)
	require.Equal(t, "stop", tracker3.finishReason)
}

func TestResponsesStreamingResponseCompletedIsSuccessful(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, "/v1/responses", r.URL.Path)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"status\":\"in_progress\"}}\n\n")
		_, _ = io.WriteString(w, "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n")
		_, _ = io.WriteString(w, "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n")
	}))
	defer server.Close()

	store := newTestStoreForHandler(t)
	ch := &channel.Channel{
		Name: "responses", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeResponses,
		Endpoint: server.URL, APIKey: "key", PrimaryModel: "gpt-test", Enabled: true,
		LastStatus: probe.StatusOperational,
	}
	require.NoError(t, store.Save(context.Background(), ch))
	h := NewHandler(store, channel.NewProbeRunner(store))
	r := gin.New()
	r.POST("/v1/responses", h.forwardResponses)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/v1/responses", strings.NewReader(`{"model":"gpt-test","stream":true,"input":"hello"}`))
	r.ServeHTTP(rec, req)
	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	require.Contains(t, rec.Body.String(), "response.completed")

	loaded, err := store.Get(context.Background(), ch.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusOperational, loaded.LastStatus)
	require.Equal(t, 0, loaded.FailureCount)
	require.Nil(t, loaded.CooldownUntil)
}

func TestParseUpstreamErrorMessage(t *testing.T) {
	// OpenAI error json
	err1 := parseUpstreamErrorMessage(`{"error":{"message":"Rate limit exceeded: 30 requests per minute"}}`)
	require.Equal(t, "Rate limit exceeded: 30 requests per minute", err1)

	// Generic error json
	err2 := parseUpstreamErrorMessage(`{"message":"Unauthorized token"}`)
	require.Equal(t, "Unauthorized token", err2)

	// Plain text
	err3 := parseUpstreamErrorMessage(`Service Unavailable`)
	require.Equal(t, "Service Unavailable", err3)
}

func TestCopyClientHeadersOnlyForwardsProtocolMetadata(t *testing.T) {
	src := http.Header{
		"Accept":                {"text/event-stream"},
		"Anthropic-Version":     {"2023-06-01"},
		"Anthropic-Beta":        {"feature-x"},
		"Authorization":         {"client-secret"},
		"Cookie":                {"session=private"},
		"Origin":                {"https://client.example"},
		"Referer":               {"https://client.example/page"},
		"X-Private-Client-Meta": {"private"},
	}
	dst := make(http.Header)
	copyClientHeaders(dst, src)
	require.Equal(t, "text/event-stream", dst.Get("Accept"))
	require.Equal(t, "2023-06-01", dst.Get("Anthropic-Version"))
	require.Equal(t, "feature-x", dst.Get("Anthropic-Beta"))
	for _, key := range []string{"Authorization", "Cookie", "Origin", "Referer", "X-Private-Client-Meta"} {
		require.Empty(t, dst.Get(key), "%s should not be forwarded", key)
	}
}

func TestIsClientClosedConn(t *testing.T) {
	require.True(t, isClientClosedConn(context.Canceled))
	require.True(t, isClientClosedConn(errors.New("write: broken pipe")))
	require.True(t, isClientClosedConn(errors.New("read: connection reset by peer")))
	require.False(t, isClientClosedConn(errors.New("dial tcp: i/o timeout")))
}

func TestEstimatePromptTokens(t *testing.T) {
	body := []byte(`{"messages":[{"role":"user","content":"Hello world, please help me write Go code"}]}`)
	tokens := estimatePromptTokens(body)
	require.Greater(t, tokens, 5)
}

func TestStreamLogsSSE(t *testing.T) {
	gin.SetMode(gin.TestMode)
	store, err := channel.OpenStore("file:desktop_stream_test?mode=memory&cache=shared")
	require.NoError(t, err)
	defer func() { _ = store.Close() }()

	runner := channel.NewProbeRunner(store)
	h := NewHandlerWithGatewayKey(store, runner, "test-key")

	r := gin.New()
	h.RegisterRoutes(r.Group("/api"))

	server := httptest.NewServer(r)
	defer server.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, server.URL+"/api/logs/stream", nil)
	require.NoError(t, err)

	resp, err := http.DefaultClient.Do(req)
	require.NoError(t, err)
	defer func() { _ = resp.Body.Close() }()

	require.Equal(t, http.StatusOK, resp.StatusCode)
	require.Contains(t, resp.Header.Get("Content-Type"), "text/event-stream")

	// Trigger a log event
	testLog := &channel.RequestLog{
		Model:       "test-stream-model",
		ChannelName: "TestStreamChannel",
		StatusCode:  0,
	}
	err = store.RecordLog(context.Background(), testLog)
	require.NoError(t, err)

	buf := make([]byte, 1024)
	n, err := resp.Body.Read(buf)
	require.NoError(t, err)
	output := string(buf[:n])
	require.Contains(t, output, "connected")

	// Read next chunk for the created event
	n, err = resp.Body.Read(buf)
	require.NoError(t, err)
	output = string(buf[:n])
	require.Contains(t, output, "log_created")
	require.Contains(t, output, "test-stream-model")
}

// The edit path must apply configuration without disturbing runtime health
// state. This checks the fields a request carries; the "landed while the
// request was in flight" half of the race is covered by
// channel.TestCopyConfigStateLeavesRuntimeAlone and
// channel.TestProbeNowDoesNotClobberConcurrentEdit, which exercise the two
// building blocks the handler relies on.
func TestUpdateAppliesConfigAndKeepsRuntimeState(t *testing.T) {
	gin.SetMode(gin.TestMode)
	store := newTestStoreForHandler(t)
	ch := &channel.Channel{
		Name: "original", Provider: probe.ProviderOpenAI, Endpoint: "https://a.example",
		APIKey: "sk-original", PrimaryModel: "m1", Enabled: true, IntervalSeconds: 300,
		LastStatus: probe.StatusOperational,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	handler := NewHandlerWithGatewayKey(store, channel.NewProbeRunner(store), "key")

	// Simulate a probe landing between the handler's read and its write by
	// changing runtime state right before issuing the request.
	runtimeCtx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	if err := store.UpdateRuntime(runtimeCtx, ch.ID, func(fresh *channel.Channel) {
		fresh.LastStatus = probe.StatusDegraded
		fresh.LastError = "probe found it slow"
		fresh.FailureCount = 3
		fresh.CircuitState = channel.CircuitOpen
	}); err != nil {
		t.Fatal(err)
	}

	body := `{"name":"renamed","provider":"openai","endpoint":"https://a.example","api_key":"","primary_model":"m1","enabled":true,"interval_seconds":300}`
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPut, "/api/channels/"+strconv.FormatInt(ch.ID, 10), strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	c, _ := gin.CreateTestContext(rec)
	c.Request = req
	c.Params = gin.Params{{Key: "id", Value: strconv.FormatInt(ch.ID, 10)}}
	handler.update(c)

	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	require.Equal(t, "renamed", loaded.Name, "config change must apply")
	require.Equal(t, probe.StatusDegraded, loaded.LastStatus, "runtime status must survive the edit")
	require.Equal(t, "probe found it slow", loaded.LastError)
	require.Equal(t, 3, loaded.FailureCount)
	require.Equal(t, channel.CircuitOpen, loaded.CircuitState)
	require.Equal(t, "sk-original", loaded.APIKey, "blank key must keep the stored one")
}

func newTestStoreForHandler(t *testing.T) *channel.Store {
	t.Helper()
	db, err := sql.Open("sqlite", "file:desktop_handler_update_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })
	store, err := channel.NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	return store
}

// The streaming token tracker splits incoming bytes on newlines and buffers
// the trailing partial line. A multi-byte UTF-8 sequence can never straddle
// that split: every byte of a rune is >= 0x80 while '\n' is 0x0A, so the
// split point is always outside a rune. Feeding a stream one byte at a time
// must therefore produce the same estimate as feeding it whole.
func TestStreamTokenTrackerIsUTF8SplitSafe(t *testing.T) {
	// Chinese text plus an emoji, both multi-byte, inside JSON string values.
	payload := `data: {"choices":[{"delta":{"content":"你好世界，这是测试"}}]}` + "\n" +
		`data: {"choices":[{"delta":{"content":"🚀 emoji 混排"}}]}` + "\n" +
		"data: [DONE]\n\n"

	whole := &streamTokenTracker{}
	whole.Feed([]byte(payload))

	byByte := &streamTokenTracker{}
	for i := 0; i < len(payload); i++ {
		byByte.Feed([]byte(payload[i : i+1]))
	}

	if byByte.completionTokens != whole.completionTokens {
		t.Fatalf("byte-at-a-time estimate differs: %d vs %d", byByte.completionTokens, whole.completionTokens)
	}
	if byByte.estimatedChars != whole.estimatedChars {
		t.Fatalf("char count differs: %d vs %d", byByte.estimatedChars, whole.estimatedChars)
	}
	if !byByte.sawDone {
		t.Fatal("byte-at-a-time feed lost the [DONE] sentinel")
	}
	if whole.completionTokens == 0 {
		t.Fatal("expected a non-zero estimate")
	}
}

// The claude-* fallback must not hand a working request to a channel that is
// benched. The old rule took the first enabled channel whatever its health.
func TestResolveModelAliasPrefersHealthyChannel(t *testing.T) {
	store := newTestStoreForHandler(t)
	cooldown := time.Now().Add(10 * time.Minute)

	broken := &channel.Channel{
		Name: "broken", Provider: probe.ProviderOpenAI, Endpoint: "https://broken.example",
		APIKey: "k1", PrimaryModel: "broken-model", Enabled: true,
		LastStatus: probe.StatusError, CircuitState: channel.CircuitOpen,
	}
	benched := &channel.Channel{
		Name: "benched", Provider: probe.ProviderOpenAI, Endpoint: "https://benched.example",
		APIKey: "k2", PrimaryModel: "benched-model", Enabled: true,
		LastStatus: probe.StatusOperational, CooldownUntil: &cooldown,
	}
	healthy := &channel.Channel{
		Name: "healthy", Provider: probe.ProviderOpenAI, Endpoint: "https://healthy.example",
		APIKey: "k3", PrimaryModel: "healthy-model", Enabled: true,
		LastStatus: probe.StatusOperational,
	}
	for _, c := range []*channel.Channel{broken, benched, healthy} {
		if err := store.Save(context.Background(), c); err != nil {
			t.Fatal(err)
		}
	}

	handler := NewHandlerWithGatewayKey(store, channel.NewProbeRunner(store), "key")
	got := handler.resolveModelAlias(context.Background(), "claude-sonnet-4-6")
	require.Equal(t, "healthy-model", got, "must skip the open-circuit and cooling-down channels")

	// With every channel benched it still returns something rather than failing.
	if err := store.UpdateRuntime(context.Background(), healthy.ID, func(fresh *channel.Channel) {
		fresh.CircuitState = channel.CircuitOpen
		fresh.LastStatus = probe.StatusError
	}); err != nil {
		t.Fatal(err)
	}
	if got := handler.resolveModelAlias(context.Background(), "claude-sonnet-4-6"); got == "" || got == "claude-sonnet-4-6" {
		t.Fatalf("expected a last-resort model, got %q", got)
	}
}

// TestUpstreamErrorPayloadPolicy locks the whole shape matrix to the policy
// documented on upstreamErrorPayload: a body is an error only when `error` is a
// non-empty value, or the top-level `code` is an HTTP status outside 2xx. A bare
// top-level `message` is not a signal. Two failure modes matter equally here — a
// false positive fails over a healthy channel and benches it, a false negative
// hands the client an empty answer — so both directions are pinned.
func TestUpstreamErrorPayloadPolicy(t *testing.T) {
	cases := []struct {
		body       string
		wantErr    bool
		wantStatus int
		why        string
	}{
		// No signal at all.
		{`{"choices":[{"delta":{"content":"hi"}}]}`, false, 0, "normal stream frame"},
		{`{"type":"message_start","message":{"id":"m1"}}`, false, 0, "anthropic frame, no error key"},

		// Empty error slots — upstreams include these routinely.
		{`{"error":null}`, false, 0, "null"},
		{`{"error":{}}`, false, 0, "empty object"},
		{`{"error":{ }}`, false, 0, "empty object, whitespace"},
		{`{"error":[]}`, false, 0, "empty array"},
		{`{"error":""}`, false, 0, "empty string"},
		{`{"error":false}`, false, 0, "false idiom"},
		{`{"error":0}`, false, 0, "numeric zero"},
		{`{"error":{"type":"","message":""}}`, false, 0, "object with all-empty values"},
		{`{"error":{},"choices":[{"delta":{"content":"x"}}]}`, false, 0, "empty slot beside content"},

		// A bare message is informational, not a failure signal.
		{`{"error":null,"message":"served by node-3"}`, false, 0, "explicit null + info message"},
		{`{"error":{},"message":"served by node-3"}`, false, 0, "empty slot + info message"},
		{`{"error":{},"message":{"id":"msg_1"}}`, false, 0, "message is an object"},
		{`{"message":"rate limited"}`, false, 0, "message with no code"},

		// Top-level code: only a 4xx/5xx HTTP status is a signal.
		{`{"code":0}`, false, 0, "0 is a success code"},
		{`{"code":"0"}`, false, 0, "0 as a string"},
		{`{"code":200}`, false, 0, "2xx"},
		{`{"code":199}`, false, 0, "1xx is not a failure"},
		{`{"code":301}`, false, 0, "3xx is not a failure"},
		{`{"code":301,"data":{"x":1}}`, false, 0, "business code in the 3xx range"},
		{`{"code":1000}`, false, 0, "outside the HTTP range"},
		{`{"code":"success"}`, false, 0, "non-numeric code"},

		// Non-empty error values.
		{`{"error":{"message":"rate limited","type":"rate_limit_error"}}`, true, 429, "typed rate limit"},
		{`{"error":{"message":"bad key","type":"invalid_api_key"}}`, true, 401, "typed auth failure"},
		{`{"error":"boom"}`, true, 502, "string error"},
		{`{"error":{"type":"overloaded_error"}}`, true, 502, "type only"},
		{`{"error":{"message":"x"}}`, true, 502, "message only"},

		// Top-level code in the failure range, with or without an error slot.
		{`{"error":{},"code":429}`, true, 429, "empty slot + code"},
		{`{"error":0,"code":429}`, true, 429, "zero slot + code"},
		{`{"code":429,"message":"rate limited"}`, true, 429, "no error key at all"},
		{`{"code":"429"}`, true, 429, "numeric string code"},
		{`{"code":429.0}`, true, 429, "integral number code"},
		{`{"code":404}`, true, 404, "4xx code"},
		{`{"code":503}`, true, 503, "5xx code"},
		{`{"error":{},"message":"rate limited","code":429}`, true, 429, "code wins over text"},

		// Nested error.code is as authoritative as the top-level one. Getting
		// this wrong maps an auth failure to 502, whose cooldown is 15s instead
		// of the 10 minutes an auth failure is meant to get.
		{`{"error":{"code":401}}`, true, 401, "nested numeric code"},
		{`{"error":{"code":429}}`, true, 429, "nested rate-limit code"},
		{`{"error":{"code":"429"}}`, true, 429, "nested numeric string code"},
		{`{"error":{"message":"x","code":503}}`, true, 503, "nested code beside message"},
		{`{"error":{"code":"invalid_api_key"}}`, true, 401, "non-numeric code still classified by text"},
		{`{"error":{"code":0,"message":"boom"}}`, true, 502, "nested zero code is not a status"},
		{`{"error":{"code":301,"message":"boom"}}`, true, 502, "nested 3xx is not a failure status"},

		// OpenAI Responses API nests the failure under `response`.
		{`{"type":"response.failed","response":{"error":{"message":"boom","code":"server_error"}}}`, true, 502, "responses failure"},
		{`{"type":"response.failed","response":{"error":{"code":429,"message":"slow down"}}}`, true, 429, "responses failure with code"},
		{`{"type":"response.completed","response":{"id":"r1","status":"completed","error":null}}`, false, 0, "responses success with null error"},
		{`{"type":"response.created","response":{"id":"r1","status":"in_progress"}}`, false, 0, "responses in-progress"},
		{`{"response":{"code":200,"status":"completed"}}`, false, 0, "responses 2xx code"},
	}

	for _, tc := range cases {
		_, status, isErr := upstreamErrorPayload([]byte(tc.body))
		if isErr != tc.wantErr {
			t.Errorf("%s: isError=%v want %v (%s)", tc.body, isErr, tc.wantErr, tc.why)
			continue
		}
		if tc.wantErr && status != tc.wantStatus {
			t.Errorf("%s: status=%d want %d (%s)", tc.body, status, tc.wantStatus, tc.why)
		}
	}
}

// A string-valued `error` keeps its text instead of degrading to the raw JSON.
func TestParseUpstreamErrorMessageUnwrapsStringError(t *testing.T) {
	require.Equal(t, "boom", parseUpstreamErrorMessage(`{"error":"boom"}`))
	require.Equal(t, "rate limited", parseUpstreamErrorMessage(`{"error":{"message":"rate limited"}}`))
	require.Equal(t, "rate limited", parseUpstreamErrorMessage(`{"message":"rate limited"}`))
	require.Equal(t, "rate limited", parseUpstreamErrorMessage(`{"msg":"rate limited"}`))
}

// Truncation of echoed text is bounded, so a large body cannot be amplified.
func TestParseUpstreamErrorMessageTruncatesLongBody(t *testing.T) {
	long := strings.Repeat("x", 5000)
	got := parseUpstreamErrorMessage(long)
	require.Len(t, got, maxEchoedErrorText+3)
	require.True(t, strings.HasSuffix(got, "..."))

	// A stream value that is not a boolean is echoed back bounded too.
	body := `{"model":"m","stream":"` + long + `"}`
	_, _, err := requestModel([]byte(body))
	require.Error(t, err)
	require.Less(t, len(err.Error()), 400)
}

// Truncation must cut on a rune boundary: a multi-byte character split in half
// would put invalid UTF-8 into the response and the logs.
func TestTruncateEchoedKeepsValidUTF8(t *testing.T) {
	// 3 bytes per rune: some lengths make the 200-byte cut land mid-rune.
	for _, n := range []int{100, 150, 201, 500} {
		got := truncateEchoed(strings.Repeat("中", n))
		require.True(t, utf8.ValidString(got), "n=%d produced invalid UTF-8", n)
		require.LessOrEqual(t, len(got), maxEchoedErrorText+3)
	}
	// ASCII is unaffected.
	require.Equal(t, "abc", truncateEchoed("abc"))
}

// A `stream` that is not a JSON boolean is rejected with a message naming the
// field, rather than the struct-field decode error a plain `bool` gives. The
// flag decides whether the reply is translated as SSE, so it must be
// unambiguous rather than guessed at.
func TestRequestModelRejectsNonBoolStream(t *testing.T) {
	cases := []struct {
		body       string
		wantStream bool
	}{
		{`{"model":"m"}`, false},
		{`{"model":"m","stream":true}`, true},
		{`{"model":"m","stream":false}`, false},
		{`{"model":"m","stream":null}`, false},
	}
	for _, tc := range cases {
		model, stream, err := requestModel([]byte(tc.body))
		require.NoError(t, err, tc.body)
		require.Equal(t, "m", model, tc.body)
		require.Equal(t, tc.wantStream, stream, tc.body)
	}

	for _, body := range []string{
		`{"model":"m","stream":"true"}`,
		`{"model":"m","stream":1}`,
		`{"model":"m","stream":{}}`,
	} {
		_, _, err := requestModel([]byte(body))
		require.Error(t, err, body)
		require.Contains(t, err.Error(), "invalid `stream`", body)
	}

	if _, _, err := requestModel([]byte(`{"stream":true}`)); err == nil {
		t.Error("a missing model must still be rejected")
	}
}

// Regression: a normal SSE frame carrying an empty `error` slot must stream
// through as success rather than being mistaken for an in-band error, which
// would fail over to another channel and penalize this one.
func TestForwardStreamsThroughEmptyErrorSlot(t *testing.T) {
	gin.SetMode(gin.TestMode)
	var hits int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hits++
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}],\"error\":{}}\n\n")
		_, _ = io.WriteString(w, "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := namedTestStore(t, "desktop_empty_error_slot_test")
	ch := &channel.Channel{
		Name: "only", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions,
		Endpoint: server.URL, APIKey: "k", PrimaryModel: "m", Enabled: true,
		LastStatus: probe.StatusOperational,
	}
	require.NoError(t, store.Save(context.Background(), ch))

	h := NewHandler(store, channel.NewProbeRunner(store))
	r := gin.New()
	r.POST("/v1/chat/completions", h.forwardChatCompletions)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/v1/chat/completions",
		strings.NewReader(`{"model":"m","stream":true,"messages":[{"role":"user","content":"hi"}]}`))
	r.ServeHTTP(rec, req)

	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	require.Equal(t, 1, hits)
	require.Contains(t, rec.Body.String(), "hi")

	loaded, err := store.Get(context.Background(), ch.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusOperational, loaded.LastStatus, "an empty error slot must not penalize the channel")
	require.Equal(t, 0, loaded.FailureCount)
}

// The Responses API reports a failure as a `response.failed` event with the
// error nested under `response`, so it needs the same first-byte failover as a
// top-level in-band error. This is the Responses counterpart of
// TestForwardFailsOverOnInBandStreamError.
func TestForwardFailsOverOnResponsesStreamFailure(t *testing.T) {
	gin.SetMode(gin.TestMode)
	var badHits, goodHits int
	badServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		badHits++
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"upstream boom\",\"code\":429}}}\n\n")
	}))
	defer badServer.Close()
	goodServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		goodHits++
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n")
		_, _ = io.WriteString(w, "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\"}}\n\n")
	}))
	defer goodServer.Close()

	store := namedTestStore(t, "desktop_responses_failover_test")
	fastTTFT, slowTTFT := 5, 50
	bad := &channel.Channel{
		Name: "bad", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeResponses,
		Endpoint: badServer.URL, APIKey: "k1", PrimaryModel: "m", Enabled: true,
		LastStatus: probe.StatusOperational, LastTTFTMs: &fastTTFT,
	}
	good := &channel.Channel{
		Name: "good", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeResponses,
		Endpoint: goodServer.URL, APIKey: "k2", PrimaryModel: "m", Enabled: true,
		LastStatus: probe.StatusOperational, LastTTFTMs: &slowTTFT,
	}
	for _, c := range []*channel.Channel{bad, good} {
		require.NoError(t, store.Save(context.Background(), c))
	}

	h := NewHandler(store, channel.NewProbeRunner(store))
	r := gin.New()
	r.POST("/v1/responses", h.forwardResponses)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/v1/responses",
		strings.NewReader(`{"model":"m","stream":true,"input":"hi"}`))
	r.ServeHTTP(rec, req)

	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	require.Equal(t, 1, badHits, "the failing channel should have been tried once")
	require.Equal(t, 1, goodHits, "must fail over to the healthy channel")
	require.Contains(t, rec.Body.String(), "hi")
	require.NotContains(t, rec.Body.String(), "upstream boom", "the failure must not reach the client")

	loadedBad, err := store.Get(context.Background(), bad.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusError, loadedBad.LastStatus)

	loadedGood, err := store.Get(context.Background(), good.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusOperational, loadedGood.LastStatus)
}

// namedTestStore opens a store on its own shared-cache in-memory database so
// tests that record channel health don't bleed state into one another.
func namedTestStore(t *testing.T, name string) *channel.Store {
	t.Helper()
	db, err := sql.Open("sqlite", "file:"+name+"?mode=memory&cache=shared")
	require.NoError(t, err)
	t.Cleanup(func() { _ = db.Close() })
	store, err := channel.NewStore(db)
	require.NoError(t, err)
	return store
}

// B1 regression: an OpenAI-compatible upstream can report an error inside an
// HTTP 200 SSE stream (rate limit, overload). Because nothing has reached the
// client yet, forward() must fail over to a healthy sibling channel instead of
// committing the errored channel and streaming the error through.
func TestForwardFailsOverOnInBandStreamError(t *testing.T) {
	gin.SetMode(gin.TestMode)
	var badHits, goodHits int
	badServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		badHits++
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"error\":{\"message\":\"rate limit exceeded\",\"type\":\"rate_limit_error\"}}\n\n")
	}))
	defer badServer.Close()
	goodServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		goodHits++
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n")
		_, _ = io.WriteString(w, "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer goodServer.Close()

	store := namedTestStore(t, "desktop_inband_failover_test")
	fastTTFT := 5
	slowTTFT := 50
	// Latency routing tries the faster (bad) channel first.
	bad := &channel.Channel{
		Name: "bad", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions,
		Endpoint: badServer.URL, APIKey: "k1", PrimaryModel: "shared-model", Enabled: true,
		LastStatus: probe.StatusOperational, LastTTFTMs: &fastTTFT,
	}
	good := &channel.Channel{
		Name: "good", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions,
		Endpoint: goodServer.URL, APIKey: "k2", PrimaryModel: "shared-model", Enabled: true,
		LastStatus: probe.StatusOperational, LastTTFTMs: &slowTTFT,
	}
	for _, c := range []*channel.Channel{bad, good} {
		require.NoError(t, store.Save(context.Background(), c))
	}

	h := NewHandler(store, channel.NewProbeRunner(store))
	r := gin.New()
	r.POST("/v1/chat/completions", h.forwardChatCompletions)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/v1/chat/completions",
		strings.NewReader(`{"model":"shared-model","stream":true,"messages":[{"role":"user","content":"hi"}]}`))
	r.ServeHTTP(rec, req)

	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	require.Equal(t, 1, badHits, "bad channel should have been tried once")
	require.Equal(t, 1, goodHits, "must fail over to the healthy channel")
	require.Contains(t, rec.Body.String(), "hi")
	require.NotContains(t, rec.Body.String(), "rate limit", "the in-band error must not reach the client")

	loadedBad, err := store.Get(context.Background(), bad.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusError, loadedBad.LastStatus, "the errored channel must be penalized")
	require.NotNil(t, loadedBad.CooldownUntil)

	loadedGood, err := store.Get(context.Background(), good.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusOperational, loadedGood.LastStatus)
	require.Equal(t, 0, loadedGood.FailureCount)
}

// B2 regression: a non-streaming request whose body merely mentions the literal
// "stream":true (e.g. a prompt discussing streaming APIs) must not be treated
// as a stream. Misclassifying it would feed the plain JSON response to the SSE
// tracker, wrongly penalize the channel, and log a 502 for a successful call.
func TestForwardDoesNotMisclassifyStreamMentionAsStreaming(t *testing.T) {
	gin.SetMode(gin.TestMode)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}`)
	}))
	defer server.Close()

	store := namedTestStore(t, "desktop_stream_mention_test")
	ch := &channel.Channel{
		Name: "json", Provider: probe.ProviderOpenAI, APIMode: probe.APIModeChatCompletions,
		Endpoint: server.URL, APIKey: "key", PrimaryModel: "m", Enabled: true,
		LastStatus: probe.StatusOperational,
	}
	require.NoError(t, store.Save(context.Background(), ch))

	h := NewHandler(store, channel.NewProbeRunner(store))
	r := gin.New()
	r.POST("/v1/chat/completions", h.forwardChatCompletions)
	rec := httptest.NewRecorder()
	// stream is false, but the user content contains the literal `"stream":true`.
	req := httptest.NewRequest(http.MethodPost, "/v1/chat/completions",
		strings.NewReader(`{"model":"m","stream":false,"messages":[{"role":"user","content":"what does {\"stream\":true} do?"}]}`))
	r.ServeHTTP(rec, req)

	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	require.Contains(t, rec.Body.String(), "ok")

	loaded, err := store.Get(context.Background(), ch.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusOperational, loaded.LastStatus, "a successful non-stream request must not penalize the channel")
	require.Equal(t, 0, loaded.FailureCount)
	require.Nil(t, loaded.CooldownUntil)
}
