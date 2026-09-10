package desktopapi

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/Wei-Shaw/sub2api/internal/service"
	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
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
	response, err := forwardDesktopRequest(context.Background(), inbound, []byte(`{"model":"claude-test","messages":[]}`), &service.DesktopChannel{
		Provider: service.MonitorProviderAnthropic,
		Endpoint: server.URL,
		APIKey:   "channel-secret",
	}, desktopGatewayProtocolAnthropic, "claude-test")
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
	response, err := forwardDesktopRequest(context.Background(), inbound, []byte(anthropicPayload), &service.DesktopChannel{
		Provider:     service.MonitorProviderOpenAI,
		APIMode:      service.MonitorAPIModeChatCompletions,
		Endpoint:     server.URL,
		APIKey:       "openai-secret",
		PrimaryModel: "deepseek-v4-flash",
	}, desktopGatewayProtocolAnthropic, "deepseek-v4-flash")

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
	response, err := forwardDesktopRequest(context.Background(), inbound, []byte(anthropicPayload), &service.DesktopChannel{
		Provider:     service.MonitorProviderOpenAI,
		APIMode:      service.MonitorAPIModeChatCompletions,
		Endpoint:     server.URL,
		APIKey:       "openai-secret",
		PrimaryModel: "deepseek-v4-flash",
	}, desktopGatewayProtocolAnthropic, "deepseek-v4-flash")

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
	store, err := service.OpenDesktopChannelStore(":memory:")
	require.NoError(t, err)
	defer func() { _ = store.Close() }()
	channel := &service.DesktopChannel{
		Name: "local", Provider: service.MonitorProviderOpenAI, APIMode: service.MonitorAPIModeResponses,
		Endpoint: "https://example.com", APIKey: "secret", PrimaryModel: "gpt-test", ExtraModels: []string{"gpt-test", "claude-test"}, Enabled: true,
	}
	require.NoError(t, store.Save(context.Background(), channel))
	h := NewHandler(store, service.NewDesktopChannelProbeRunner(store))
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
	store, err := service.OpenDesktopChannelStore("file:desktop_stream_test?mode=memory&cache=shared")
	require.NoError(t, err)
	defer func() { _ = store.Close() }()

	runner := service.NewDesktopChannelProbeRunner(store)
	h := NewHandlerWithGatewayKey(store, runner, "test-key")

	r := gin.New()
	h.RegisterRoutes(r.Group("/desktop/api"))

	server := httptest.NewServer(r)
	defer server.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, server.URL+"/desktop/api/logs/stream", nil)
	require.NoError(t, err)

	resp, err := http.DefaultClient.Do(req)
	require.NoError(t, err)
	defer func() { _ = resp.Body.Close() }()

	require.Equal(t, http.StatusOK, resp.StatusCode)
	require.Contains(t, resp.Header.Get("Content-Type"), "text/event-stream")

	// Trigger a log event
	testLog := &service.DesktopRequestLog{
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
