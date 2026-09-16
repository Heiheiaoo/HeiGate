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
