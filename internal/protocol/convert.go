package protocol

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// ErrNoMessages is returned when a request carries no conversation turns.
var ErrNoMessages = errors.New("messages must not be empty")

// MessagesToChat converts an Anthropic Messages request into an OpenAI Chat
// Completions request. The mapping is:
//
//	system                     -> a leading "system" message
//	user text/image blocks     -> "user" message with content parts
//	user tool_result blocks    -> one "tool" message per result (emitted first,
//	                              because OpenAI requires tool replies to
//	                              immediately follow the assistant tool call)
//	assistant text blocks      -> "assistant" content
//	assistant tool_use blocks  -> "assistant" tool_calls
//	assistant thinking blocks  -> dropped (provider specific, not replayable)
//	tools / tool_choice        -> function tools / tool_choice
//	max_tokens, temperature, top_p, stop_sequences, metadata.user_id
func MessagesToChat(req *MessagesRequest) (*ChatRequest, error) {
	if req == nil {
		return nil, errors.New("request is nil")
	}
	if len(req.Messages) == 0 {
		return nil, ErrNoMessages
	}
	out := &ChatRequest{
		Model:       req.Model,
		Temperature: req.Temperature,
		TopP:        req.TopP,
		Stream:      req.Stream,
	}
	if req.MaxTokens > 0 {
		mt := req.MaxTokens
		out.MaxTokens = &mt
	}
	if len(req.StopSequences) > 0 {
		out.Stop = append([]string{}, req.StopSequences...)
	}
	if req.Stream {
		out.StreamOptions = &StreamOptions{IncludeUsage: true}
	}
	if req.Metadata != nil && req.Metadata.UserID != "" {
		out.User = req.Metadata.UserID
	}

	if system := SystemText(req.System); system != "" {
		out.Messages = append(out.Messages, ChatMessage{Role: "system", Content: rawString(system)})
	}

	for i, msg := range req.Messages {
		switch msg.Role {
		case "user":
			out.Messages = append(out.Messages, userTurnToChat(msg)...)
		case "assistant":
			out.Messages = append(out.Messages, assistantTurnToChat(msg))
		default:
			return nil, fmt.Errorf("messages[%d]: unsupported role %q", i, msg.Role)
		}
	}

	for _, tool := range req.Tools {
		if tool.Name == "" || len(trimRaw(tool.InputSchema)) == 0 {
			continue // server-side tools have no client schema
		}
		out.Tools = append(out.Tools, ChatTool{
			Type: "function",
			Function: FunctionSpec{
				Name:        tool.Name,
				Description: tool.Description,
				Parameters:  tool.InputSchema,
			},
		})
	}

	if req.ToolChoice != nil && len(out.Tools) > 0 {
		switch req.ToolChoice.Type {
		case "any":
			out.ToolChoice = "required"
		case "tool":
			out.ToolChoice = map[string]any{
				"type":     "function",
				"function": map[string]string{"name": req.ToolChoice.Name},
			}
		case "none":
			out.ToolChoice = "none"
		default:
			out.ToolChoice = "auto"
		}
		if req.ToolChoice.DisableParallelToolUse {
			f := false
			out.ParallelToolCalls = &f
		}
	}
	return out, nil
}

func userTurnToChat(msg Message) []ChatMessage {
	blocks := Blocks(msg.Content)
	var toolReplies []ChatMessage
	var parts []ChatContentPart
	for _, b := range blocks {
		switch b.Type {
		case "text":
			if b.Text != "" {
				parts = append(parts, ChatContentPart{Type: "text", Text: b.Text})
			}
		case "image":
			if url := imageToDataURL(b.Source); url != "" {
				parts = append(parts, ChatContentPart{Type: "image_url", ImageURL: &ImageURL{URL: url}})
			}
		case "tool_result":
			toolReplies = append(toolReplies, ChatMessage{
				Role:       "tool",
				ToolCallID: b.ToolUseID,
				Content:    rawString(toolResultText(b)),
			})
		}
	}
	out := toolReplies
	if len(parts) > 0 {
		out = append(out, ChatMessage{Role: "user", Content: encodeParts(parts)})
	} else if len(toolReplies) == 0 {
		// An empty user turn is still a turn; keep the conversation shape valid.
		out = append(out, ChatMessage{Role: "user", Content: rawString("")})
	}
	return out
}

func assistantTurnToChat(msg Message) ChatMessage {
	out := ChatMessage{Role: "assistant"}
	var text strings.Builder
	for _, b := range Blocks(msg.Content) {
		switch b.Type {
		case "text":
			text.WriteString(b.Text)
		case "tool_use":
			args := strings.TrimSpace(string(b.Input))
			if args == "" {
				args = "{}"
			}
			out.ToolCalls = append(out.ToolCalls, ToolCall{
				ID:       b.ID,
				Type:     "function",
				Function: FunctionCall{Name: b.Name, Arguments: args},
			})
		}
	}
	if text.Len() > 0 || len(out.ToolCalls) == 0 {
		out.Content = rawString(text.String())
	}
	return out
}

// encodeParts collapses a single text part into a plain string, which every
// OpenAI-compatible provider accepts, and keeps the array form otherwise.
func encodeParts(parts []ChatContentPart) json.RawMessage {
	if len(parts) == 1 && parts[0].Type == "text" {
		return rawString(parts[0].Text)
	}
	b, _ := json.Marshal(parts)
	return b
}

func imageToDataURL(src *ImageSource) string {
	if src == nil {
		return ""
	}
	switch src.Type {
	case "url":
		return src.URL
	case "base64":
		if src.Data == "" {
			return ""
		}
		mediaType := src.MediaType
		if mediaType == "" {
			mediaType = "image/png"
		}
		return "data:" + mediaType + ";base64," + src.Data
	}
	return ""
}

// toolResultText flattens a tool_result payload to text. Nested images are
// summarised because the OpenAI "tool" role only accepts text.
func toolResultText(b ContentBlock) string {
	raw := trimRaw(b.Content)
	if len(raw) == 0 {
		return ""
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return s
	}
	var sb strings.Builder
	for _, inner := range Blocks(raw) {
		switch inner.Type {
		case "text":
			sb.WriteString(inner.Text)
		case "image":
			sb.WriteString("[image omitted]")
		}
	}
	if sb.Len() == 0 {
		return string(raw)
	}
	return sb.String()
}

// ChatToMessages converts a non-streaming Chat Completions response into an
// Anthropic Messages response. model is echoed back to the client verbatim
// so that it sees the name it asked for rather than the upstream alias.
func ChatToMessages(resp *ChatResponse, model string) *MessagesResponse {
	out := &MessagesResponse{
		ID:         messageID(resp.ID),
		Type:       "message",
		Role:       "assistant",
		Model:      model,
		StopReason: StopEndTurn,
		Content:    []ContentBlock{},
	}
	if resp.Usage != nil {
		out.Usage = MessagesUsage{InputTokens: resp.Usage.PromptTokens, OutputTokens: resp.Usage.CompletionTokens}
	}
	if len(resp.Choices) == 0 {
		out.Content = append(out.Content, ContentBlock{Type: "text", Text: ""})
		return out
	}
	choice := resp.Choices[0]
	if reasoning := strings.TrimSpace(choice.Message.ReasoningContent); reasoning != "" {
		out.Content = append(out.Content, ContentBlock{Type: "thinking", Thinking: reasoning})
	}
	if text := ChatText(choice.Message.Content); text != "" {
		out.Content = append(out.Content, ContentBlock{Type: "text", Text: text})
	}
	for i, call := range choice.Message.ToolCalls {
		out.Content = append(out.Content, ContentBlock{
			Type:  "tool_use",
			ID:    toolUseID(call.ID, i),
			Name:  call.Function.Name,
			Input: argumentsToInput(call.Function.Arguments),
		})
	}
	if len(out.Content) == 0 {
		out.Content = append(out.Content, ContentBlock{Type: "text", Text: ""})
	}
	out.StopReason = stopReasonFromFinish(choice.FinishReason, len(choice.Message.ToolCalls) > 0)
	return out
}

func stopReasonFromFinish(finish string, hasToolCalls bool) string {
	switch finish {
	case FinishToolCalls:
		return StopToolUse
	case FinishLength:
		return StopMaxTokens
	case FinishStop, FinishContentFilter, "":
		if hasToolCalls {
			return StopToolUse
		}
		return StopEndTurn
	default:
		if hasToolCalls {
			return StopToolUse
		}
		return StopEndTurn
	}
}

// argumentsToInput parses the JSON arguments string; malformed or empty
// arguments become an empty object so the client always receives valid JSON.
func argumentsToInput(args string) json.RawMessage {
	args = strings.TrimSpace(args)
	if args == "" {
		return json.RawMessage("{}")
	}
	var probe any
	if err := json.Unmarshal([]byte(args), &probe); err != nil {
		return json.RawMessage("{}")
	}
	if _, isObject := probe.(map[string]any); !isObject {
		return json.RawMessage("{}")
	}
	return json.RawMessage(args)
}

func messageID(id string) string {
	id = strings.TrimSpace(id)
	if id == "" {
		return "msg_" + randomID(12)
	}
	if strings.HasPrefix(id, "msg_") {
		return id
	}
	return "msg_" + id
}

func toolUseID(id string, index int) string {
	id = strings.TrimSpace(id)
	if id != "" {
		return id
	}
	return fmt.Sprintf("toolu_%s_%d", randomID(8), index)
}
