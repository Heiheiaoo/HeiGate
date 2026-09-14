package protocol

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
)

// Event is one Anthropic streaming event. The "type" key is always present.
type Event map[string]any

// Type returns the event type ("message_start", "content_block_delta", ...).
func (e Event) Type() string {
	t, _ := e["type"].(string)
	return t
}

// EncodeSSE renders an event as a server-sent-event frame:
//
//	event: <type>\n
//	data: <json>\n\n
func EncodeSSE(e Event) (string, error) {
	if e.Type() == "" {
		return "", fmt.Errorf("event has no type")
	}
	data, err := json.Marshal(e)
	if err != nil {
		return "", err
	}
	return "event: " + e.Type() + "\ndata: " + string(data) + "\n\n", nil
}

// StreamState converts a sequence of OpenAI chat chunks into Anthropic
// streaming events. Create one per response with NewStreamState, call Feed
// for every chunk, then Finish once the upstream stream ends.
//
// Block layout produced:
//
//	[thinking block]* [text block]* [tool_use block]* ... interleaved as the
//	upstream emits them; a new block is opened whenever the kind changes or a
//	tool call with a new index appears.
type StreamState struct {
	model        string
	messageID    string
	started      bool
	finished     bool
	nextIndex    int
	openIndex    int    // index of the currently open block, -1 when none
	openKind     string // "text" | "thinking" | "tool_use"
	openToolIdx  int    // upstream tool_calls[].index of the open tool block
	toolBlocks   map[int]int
	inputTokens  int
	outputTokens int
	sawUsage     bool
	stopReason   string
	sawToolCalls bool
}

// NewStreamState prepares a converter that reports the given model name to
// the client.
func NewStreamState(model string) *StreamState {
	return &StreamState{model: model, openIndex: -1, openToolIdx: -1, toolBlocks: make(map[int]int)}
}

// Feed consumes one chunk and returns the events it produced (possibly none).
func (s *StreamState) Feed(chunk *ChatChunk) []Event {
	if s.finished || chunk == nil {
		return nil
	}
	var events []Event
	if !s.started {
		if s.messageID == "" {
			s.messageID = messageID(chunk.ID)
		}
		events = append(events, s.startEvent())
	}
	if chunk.Usage != nil {
		s.sawUsage = true
		if chunk.Usage.PromptTokens > 0 {
			s.inputTokens = chunk.Usage.PromptTokens
		}
		if chunk.Usage.CompletionTokens > 0 {
			s.outputTokens = chunk.Usage.CompletionTokens
		}
	}
	for _, choice := range chunk.Choices {
		if choice.Index != 0 {
			continue // only the first candidate is streamed to the client
		}
		if reasoning := choice.Delta.ReasoningContent; reasoning != "" {
			events = append(events, s.ensureBlock("thinking", nil)...)
			events = append(events, Event{"type": "content_block_delta", "index": s.openIndex,
				"delta": map[string]any{"type": "thinking_delta", "thinking": reasoning}})
			s.countTokens(reasoning)
		}
		if text := ChatText(choice.Delta.Content); text != "" {
			events = append(events, s.ensureBlock("text", nil)...)
			events = append(events, Event{"type": "content_block_delta", "index": s.openIndex,
				"delta": map[string]any{"type": "text_delta", "text": text}})
			s.countTokens(text)
		}
		for i, call := range choice.Delta.ToolCalls {
			s.sawToolCalls = true
			idx := i
			if call.Index != nil {
				idx = *call.Index
			}
			if _, known := s.toolBlocks[idx]; !known {
				events = append(events, s.ensureBlock("tool_use", &call)...)
				s.toolBlocks[idx] = s.openIndex
				s.openToolIdx = idx
			}
			if args := call.Function.Arguments; args != "" {
				events = append(events, Event{"type": "content_block_delta", "index": s.toolBlocks[idx],
					"delta": map[string]any{"type": "input_json_delta", "partial_json": args}})
				s.countTokens(args)
			}
		}
		if choice.FinishReason != nil && *choice.FinishReason != "" {
			s.stopReason = stopReasonFromFinish(*choice.FinishReason, s.sawToolCalls)
		}
	}
	return events
}

// Finish closes any open block and emits message_delta + message_stop. It
// is safe to call even if no chunk was ever fed; the client then receives a
// well-formed empty message.
func (s *StreamState) Finish() []Event {
	if s.finished {
		return nil
	}
	s.finished = true
	var events []Event
	if !s.started {
		if s.messageID == "" {
			s.messageID = messageID("")
		}
		events = append(events, s.startEvent())
	}
	events = append(events, s.closeBlock()...)
	stop := s.stopReason
	if stop == "" {
		if s.sawToolCalls {
			stop = StopToolUse
		} else {
			stop = StopEndTurn
		}
	}
	events = append(events,
		Event{"type": "message_delta",
			"delta": map[string]any{"stop_reason": stop, "stop_sequence": nil},
			"usage": map[string]any{"output_tokens": s.outputTokens}},
		Event{"type": "message_stop"},
	)
	return events
}

// Usage returns the token counts observed so far. exact is true when the
// upstream reported usage itself rather than HeiGate estimating it.
func (s *StreamState) Usage() (input, output int, exact bool) {
	return s.inputTokens, s.outputTokens, s.sawUsage
}

func (s *StreamState) startEvent() Event {
	s.started = true
	return Event{"type": "message_start", "message": map[string]any{
		"id":            s.messageID,
		"type":          "message",
		"role":          "assistant",
		"model":         s.model,
		"content":       []any{},
		"stop_reason":   nil,
		"stop_sequence": nil,
		"usage":         map[string]any{"input_tokens": s.inputTokens, "output_tokens": 0},
	}}
}

// ensureBlock opens a block of the requested kind if the current open block
// is of a different kind (or none is open). For tool_use the call supplies
// id and name.
func (s *StreamState) ensureBlock(kind string, call *ToolCall) []Event {
	if s.openIndex >= 0 && s.openKind == kind && kind != "tool_use" {
		return nil
	}
	events := s.closeBlock()
	index := s.nextIndex
	s.nextIndex++
	s.openIndex = index
	s.openKind = kind
	block := map[string]any{"type": kind}
	switch kind {
	case "text":
		block["text"] = ""
	case "thinking":
		block["thinking"] = ""
	case "tool_use":
		id := ""
		name := ""
		if call != nil {
			id = call.ID
			name = call.Function.Name
		}
		block["id"] = toolUseID(id, index)
		block["name"] = name
		block["input"] = map[string]any{}
	}
	events = append(events, Event{"type": "content_block_start", "index": index, "content_block": block})
	return events
}

func (s *StreamState) closeBlock() []Event {
	if s.openIndex < 0 {
		return nil
	}
	ev := Event{"type": "content_block_stop", "index": s.openIndex}
	s.openIndex = -1
	s.openKind = ""
	s.openToolIdx = -1
	return []Event{ev}
}

// countTokens keeps a rough running output estimate until the upstream
// reports exact usage: CJK characters count as one token each, other text as
// roughly four characters per token.
func (s *StreamState) countTokens(text string) {
	if s.sawUsage {
		return
	}
	s.outputTokens += EstimateTokens(text)
}

// EstimateTokens is a cheap heuristic used when a provider omits usage.
func EstimateTokens(text string) int {
	if text == "" {
		return 0
	}
	wide := 0
	ascii := 0
	for _, r := range text {
		if r > 0x7F {
			wide++
		} else {
			ascii++
		}
	}
	n := wide + (ascii+3)/4
	if n < 1 {
		n = 1
	}
	return n
}

// ParseSSELine extracts the JSON payload from a "data: ..." line. done is
// true for the OpenAI "[DONE]" sentinel; ok is false for non-data lines.
func ParseSSELine(line string) (payload string, done bool, ok bool) {
	line = strings.TrimSpace(line)
	if !strings.HasPrefix(line, "data:") {
		return "", false, false
	}
	payload = strings.TrimSpace(strings.TrimPrefix(line, "data:"))
	if payload == "[DONE]" {
		return "", true, true
	}
	if payload == "" {
		return "", false, false
	}
	return payload, false, true
}

func randomID(n int) string {
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		return strings.Repeat("0", n*2)
	}
	return hex.EncodeToString(buf)
}
