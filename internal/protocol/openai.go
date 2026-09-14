package protocol

import "encoding/json"

// ---------------------------------------------------------------------------
// OpenAI Chat Completions API
// ---------------------------------------------------------------------------

// ChatRequest is the body of POST /v1/chat/completions.
type ChatRequest struct {
	Model             string         `json:"model"`
	Messages          []ChatMessage  `json:"messages"`
	MaxTokens         *int           `json:"max_tokens,omitempty"`
	Temperature       *float64       `json:"temperature,omitempty"`
	TopP              *float64       `json:"top_p,omitempty"`
	Stop              []string       `json:"stop,omitempty"`
	Stream            bool           `json:"stream,omitempty"`
	StreamOptions     *StreamOptions `json:"stream_options,omitempty"`
	Tools             []ChatTool     `json:"tools,omitempty"`
	ToolChoice        any            `json:"tool_choice,omitempty"`
	ParallelToolCalls *bool          `json:"parallel_tool_calls,omitempty"`
	User              string         `json:"user,omitempty"`
}

// StreamOptions asks the upstream to append a usage chunk to the stream.
type StreamOptions struct {
	IncludeUsage bool `json:"include_usage"`
}

// ChatMessage is one message in the OpenAI format. Content is a string or
// an array of ChatContentPart; it is omitted entirely for tool-call-only
// assistant turns.
type ChatMessage struct {
	Role             string          `json:"role"`
	Content          json.RawMessage `json:"content,omitempty"`
	Name             string          `json:"name,omitempty"`
	ToolCalls        []ToolCall      `json:"tool_calls,omitempty"`
	ToolCallID       string          `json:"tool_call_id,omitempty"`
	ReasoningContent string          `json:"reasoning_content,omitempty"`
}

// ChatContentPart is a multimodal content element.
type ChatContentPart struct {
	Type     string    `json:"type"` // text | image_url
	Text     string    `json:"text,omitempty"`
	ImageURL *ImageURL `json:"image_url,omitempty"`
}

// ImageURL wraps an http(s) or data: URL.
type ImageURL struct {
	URL    string `json:"url"`
	Detail string `json:"detail,omitempty"`
}

// ChatTool declares a function the model may call.
type ChatTool struct {
	Type     string       `json:"type"` // "function"
	Function FunctionSpec `json:"function"`
}

// FunctionSpec is the JSON-schema description of a function tool.
type FunctionSpec struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	Parameters  json.RawMessage `json:"parameters,omitempty"`
}

// ToolCall is a function invocation requested by the assistant. Index is
// only present in streaming deltas.
type ToolCall struct {
	Index    *int         `json:"index,omitempty"`
	ID       string       `json:"id,omitempty"`
	Type     string       `json:"type,omitempty"`
	Function FunctionCall `json:"function"`
}

// FunctionCall names the function and carries its JSON-encoded arguments.
type FunctionCall struct {
	Name      string `json:"name,omitempty"`
	Arguments string `json:"arguments,omitempty"`
}

// ChatResponse is a non-streaming completion.
type ChatResponse struct {
	ID      string       `json:"id"`
	Object  string       `json:"object,omitempty"`
	Created int64        `json:"created,omitempty"`
	Model   string       `json:"model,omitempty"`
	Choices []ChatChoice `json:"choices"`
	Usage   *ChatUsage   `json:"usage,omitempty"`
}

// ChatChoice is one candidate answer.
type ChatChoice struct {
	Index        int         `json:"index"`
	Message      ChatMessage `json:"message"`
	FinishReason string      `json:"finish_reason,omitempty"`
}

// ChatChunk is one streaming SSE payload.
type ChatChunk struct {
	ID      string        `json:"id"`
	Object  string        `json:"object,omitempty"`
	Created int64         `json:"created,omitempty"`
	Model   string        `json:"model,omitempty"`
	Choices []ChunkChoice `json:"choices"`
	Usage   *ChatUsage    `json:"usage,omitempty"`
}

// ChunkChoice carries an incremental delta.
type ChunkChoice struct {
	Index        int         `json:"index"`
	Delta        ChatMessage `json:"delta"`
	FinishReason *string     `json:"finish_reason"`
}

// ChatUsage is the OpenAI token accounting block.
type ChatUsage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
	TotalTokens      int `json:"total_tokens"`
}

// OpenAI finish reasons.
const (
	FinishStop          = "stop"
	FinishLength        = "length"
	FinishToolCalls     = "tool_calls"
	FinishContentFilter = "content_filter"
)

// ChatText flattens a ChatMessage.Content value (string or parts) to text.
func ChatText(raw json.RawMessage) string {
	raw = trimRaw(raw)
	if len(raw) == 0 || string(raw) == "null" {
		return ""
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return s
	}
	var parts []ChatContentPart
	if json.Unmarshal(raw, &parts) == nil {
		var out []byte
		for _, p := range parts {
			if p.Type == "text" || p.Type == "" {
				out = append(out, p.Text...)
			}
		}
		return string(out)
	}
	return ""
}

func rawString(s string) json.RawMessage {
	b, _ := json.Marshal(s)
	return b
}
