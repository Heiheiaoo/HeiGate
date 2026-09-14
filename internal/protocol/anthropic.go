// Package protocol translates between the Anthropic Messages API and the
// OpenAI Chat Completions API so that a client speaking one dialect can be
// served by an upstream that speaks the other. Only the subset needed by a
// coding-assistant gateway is modelled: text, images, tool definitions, tool
// calls, tool results, thinking/reasoning text, stop reasons and usage.
//
// Wire formats follow the public documentation of both APIs:
//   - https://docs.anthropic.com/en/api/messages
//   - https://platform.openai.com/docs/api-reference/chat
package protocol

import (
	"encoding/json"
	"strings"
)

// ---------------------------------------------------------------------------
// Anthropic Messages API
// ---------------------------------------------------------------------------

// MessagesRequest is the body of POST /v1/messages.
type MessagesRequest struct {
	Model         string          `json:"model"`
	Messages      []Message       `json:"messages"`
	System        json.RawMessage `json:"system,omitempty"` // string or []ContentBlock
	MaxTokens     int             `json:"max_tokens,omitempty"`
	Temperature   *float64        `json:"temperature,omitempty"`
	TopP          *float64        `json:"top_p,omitempty"`
	TopK          *int            `json:"top_k,omitempty"`
	StopSequences []string        `json:"stop_sequences,omitempty"`
	Stream        bool            `json:"stream,omitempty"`
	Tools         []Tool          `json:"tools,omitempty"`
	ToolChoice    *ToolChoice     `json:"tool_choice,omitempty"`
	Metadata      *Metadata       `json:"metadata,omitempty"`
	Thinking      json.RawMessage `json:"thinking,omitempty"`
}

// Message is one conversation turn. Content is either a plain string or an
// array of ContentBlock.
type Message struct {
	Role    string          `json:"role"`
	Content json.RawMessage `json:"content"`
}

// ContentBlock is a union of every block type we understand. Unused fields
// are omitted when marshalled.
type ContentBlock struct {
	Type string `json:"type"`

	// text / thinking
	Text      string `json:"text,omitempty"`
	Thinking  string `json:"thinking,omitempty"`
	Signature string `json:"signature,omitempty"`

	// image
	Source *ImageSource `json:"source,omitempty"`

	// tool_use
	ID    string          `json:"id,omitempty"`
	Name  string          `json:"name,omitempty"`
	Input json.RawMessage `json:"input,omitempty"`

	// tool_result
	ToolUseID string          `json:"tool_use_id,omitempty"`
	Content   json.RawMessage `json:"content,omitempty"` // string or []ContentBlock
	IsError   bool            `json:"is_error,omitempty"`

	CacheControl json.RawMessage `json:"cache_control,omitempty"`
}

// ImageSource carries either inline base64 data or a URL.
type ImageSource struct {
	Type      string `json:"type"` // "base64" | "url"
	MediaType string `json:"media_type,omitempty"`
	Data      string `json:"data,omitempty"`
	URL       string `json:"url,omitempty"`
}

// Tool is a client-defined function tool. Server-side tools (web search etc.)
// carry a Type and no InputSchema and are ignored during translation.
type Tool struct {
	Type        string          `json:"type,omitempty"`
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	InputSchema json.RawMessage `json:"input_schema,omitempty"`
}

// ToolChoice controls how the model may use tools.
type ToolChoice struct {
	Type                   string `json:"type"` // auto | any | tool | none
	Name                   string `json:"name,omitempty"`
	DisableParallelToolUse bool   `json:"disable_parallel_tool_use,omitempty"`
}

// Metadata carries the optional end-user identifier.
type Metadata struct {
	UserID string `json:"user_id,omitempty"`
}

// MessagesResponse is the body returned by POST /v1/messages.
type MessagesResponse struct {
	ID           string         `json:"id"`
	Type         string         `json:"type"`
	Role         string         `json:"role"`
	Model        string         `json:"model"`
	Content      []ContentBlock `json:"content"`
	StopReason   string         `json:"stop_reason"`
	StopSequence *string        `json:"stop_sequence"`
	Usage        MessagesUsage  `json:"usage"`
}

// MessagesUsage is the Anthropic token accounting block.
type MessagesUsage struct {
	InputTokens              int `json:"input_tokens"`
	OutputTokens             int `json:"output_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
}

// Anthropic stop reasons.
const (
	StopEndTurn      = "end_turn"
	StopMaxTokens    = "max_tokens"
	StopStopSequence = "stop_sequence"
	StopToolUse      = "tool_use"
)

// Blocks decodes a Message.Content (or tool_result content) value into
// blocks. A plain string becomes a single text block.
func Blocks(raw json.RawMessage) []ContentBlock {
	raw = trimRaw(raw)
	if len(raw) == 0 || string(raw) == "null" {
		return nil
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		if s == "" {
			return nil
		}
		return []ContentBlock{{Type: "text", Text: s}}
	}
	var blocks []ContentBlock
	if json.Unmarshal(raw, &blocks) == nil {
		return blocks
	}
	return nil
}

// SystemText flattens the system prompt into a single string.
func SystemText(raw json.RawMessage) string {
	var sb strings.Builder
	for _, block := range Blocks(raw) {
		if block.Type == "text" && block.Text != "" {
			if sb.Len() > 0 {
				sb.WriteString("\n\n")
			}
			sb.WriteString(block.Text)
		}
	}
	return sb.String()
}

func trimRaw(raw json.RawMessage) json.RawMessage {
	return json.RawMessage(strings.TrimSpace(string(raw)))
}
