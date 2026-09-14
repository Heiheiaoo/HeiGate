package protocol

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"
)

func decodeRequest(t *testing.T, body string) *MessagesRequest {
	t.Helper()
	var req MessagesRequest
	require.NoError(t, json.Unmarshal([]byte(body), &req))
	return &req
}

func TestMessagesToChatBasic(t *testing.T) {
	req := decodeRequest(t, `{
		"model":"claude-x","max_tokens":100,"temperature":0.2,"stop_sequences":["END"],
		"system":"be brief",
		"messages":[{"role":"user","content":"hello"}],
		"metadata":{"user_id":"u1"}
	}`)
	chat, err := MessagesToChat(req)
	require.NoError(t, err)
	require.Equal(t, "claude-x", chat.Model)
	require.Equal(t, 100, *chat.MaxTokens)
	require.Equal(t, 0.2, *chat.Temperature)
	require.Equal(t, []string{"END"}, chat.Stop)
	require.Equal(t, "u1", chat.User)
	require.Nil(t, chat.StreamOptions)
	require.Len(t, chat.Messages, 2)
	require.Equal(t, "system", chat.Messages[0].Role)
	require.Equal(t, "be brief", ChatText(chat.Messages[0].Content))
	require.Equal(t, "user", chat.Messages[1].Role)
	require.JSONEq(t, `"hello"`, string(chat.Messages[1].Content))
}

func TestMessagesToChatSystemBlocksAndStream(t *testing.T) {
	req := decodeRequest(t, `{
		"model":"m","stream":true,
		"system":[{"type":"text","text":"one"},{"type":"text","text":"two","cache_control":{"type":"ephemeral"}}],
		"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]
	}`)
	chat, err := MessagesToChat(req)
	require.NoError(t, err)
	require.True(t, chat.Stream)
	require.NotNil(t, chat.StreamOptions)
	require.True(t, chat.StreamOptions.IncludeUsage)
	require.Equal(t, "one\n\ntwo", ChatText(chat.Messages[0].Content))
	require.JSONEq(t, `"hi"`, string(chat.Messages[1].Content), "single text part collapses to a string")
}

func TestMessagesToChatImagesBecomeParts(t *testing.T) {
	req := decodeRequest(t, `{"model":"m","messages":[{"role":"user","content":[
		{"type":"text","text":"what is this"},
		{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"AAAA"}},
		{"type":"image","source":{"type":"url","url":"https://img.example/x.png"}}
	]}]}`)
	chat, err := MessagesToChat(req)
	require.NoError(t, err)
	var parts []ChatContentPart
	require.NoError(t, json.Unmarshal(chat.Messages[0].Content, &parts))
	require.Len(t, parts, 3)
	require.Equal(t, "text", parts[0].Type)
	require.Equal(t, "data:image/jpeg;base64,AAAA", parts[1].ImageURL.URL)
	require.Equal(t, "https://img.example/x.png", parts[2].ImageURL.URL)
}

func TestMessagesToChatToolRoundTrip(t *testing.T) {
	req := decodeRequest(t, `{"model":"m","messages":[
		{"role":"user","content":"weather?"},
		{"role":"assistant","content":[
			{"type":"thinking","thinking":"let me check","signature":"sig"},
			{"type":"text","text":"Checking."},
			{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"Shanghai"}}
		]},
		{"role":"user","content":[
			{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"sunny"}]},
			{"type":"text","text":"thanks"}
		]}
	],
	"tools":[
		{"name":"get_weather","description":"Weather lookup","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}},
		{"type":"web_search_20250305","name":"web_search"}
	],
	"tool_choice":{"type":"tool","name":"get_weather","disable_parallel_tool_use":true}}`)
	chat, err := MessagesToChat(req)
	require.NoError(t, err)

	require.Len(t, chat.Messages, 4)
	assistant := chat.Messages[1]
	require.Equal(t, "assistant", assistant.Role)
	require.Equal(t, "Checking.", ChatText(assistant.Content))
	require.Len(t, assistant.ToolCalls, 1)
	require.Equal(t, "toolu_1", assistant.ToolCalls[0].ID)
	require.Equal(t, "function", assistant.ToolCalls[0].Type)
	require.Equal(t, "get_weather", assistant.ToolCalls[0].Function.Name)
	require.JSONEq(t, `{"city":"Shanghai"}`, assistant.ToolCalls[0].Function.Arguments)

	toolReply := chat.Messages[2]
	require.Equal(t, "tool", toolReply.Role)
	require.Equal(t, "toolu_1", toolReply.ToolCallID)
	require.Equal(t, "sunny", ChatText(toolReply.Content))
	require.Equal(t, "user", chat.Messages[3].Role)
	require.Equal(t, "thanks", ChatText(chat.Messages[3].Content))

	require.Len(t, chat.Tools, 1, "server-side tools are not forwarded")
	require.Equal(t, "get_weather", chat.Tools[0].Function.Name)
	require.JSONEq(t, `{"type":"object","properties":{"city":{"type":"string"}}}`, string(chat.Tools[0].Function.Parameters))
	require.Equal(t, map[string]any{"type": "function", "function": map[string]string{"name": "get_weather"}}, chat.ToolChoice)
	require.NotNil(t, chat.ParallelToolCalls)
	require.False(t, *chat.ParallelToolCalls)
}

func TestMessagesToChatToolChoiceVariants(t *testing.T) {
	base := `{"model":"m","messages":[{"role":"user","content":"x"}],"tools":[{"name":"f","input_schema":{"type":"object"}}],"tool_choice":%s}`
	for choice, want := range map[string]any{
		`{"type":"auto"}`: "auto",
		`{"type":"any"}`:  "required",
		`{"type":"none"}`: "none",
	} {
		chat, err := MessagesToChat(decodeRequest(t, strings.Replace(base, "%s", choice, 1)))
		require.NoError(t, err)
		require.Equal(t, want, chat.ToolChoice, choice)
	}
}

func TestMessagesToChatAssistantToolOnlyTurnOmitsContent(t *testing.T) {
	req := decodeRequest(t, `{"model":"m","messages":[
		{"role":"user","content":"x"},
		{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"f","input":{}}]}
	]}`)
	chat, err := MessagesToChat(req)
	require.NoError(t, err)
	raw, err := json.Marshal(chat.Messages[1])
	require.NoError(t, err)
	require.NotContains(t, string(raw), `"content"`)
	require.Contains(t, string(raw), `"tool_calls"`)
}

func TestMessagesToChatRejectsEmptyAndBadRole(t *testing.T) {
	_, err := MessagesToChat(decodeRequest(t, `{"model":"m","messages":[]}`))
	require.ErrorIs(t, err, ErrNoMessages)
	_, err = MessagesToChat(decodeRequest(t, `{"model":"m","messages":[{"role":"system","content":"x"}]}`))
	require.Error(t, err)
}

func TestChatToMessagesText(t *testing.T) {
	var resp ChatResponse
	require.NoError(t, json.Unmarshal([]byte(`{"id":"chatcmpl-9","choices":[{"index":0,"message":{"role":"assistant","content":"world"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":7,"total_tokens":12}}`), &resp))
	msg := ChatToMessages(&resp, "claude-alias")
	require.Equal(t, "msg_chatcmpl-9", msg.ID)
	require.Equal(t, "message", msg.Type)
	require.Equal(t, "assistant", msg.Role)
	require.Equal(t, "claude-alias", msg.Model)
	require.Len(t, msg.Content, 1)
	require.Equal(t, "text", msg.Content[0].Type)
	require.Equal(t, "world", msg.Content[0].Text)
	require.Equal(t, StopEndTurn, msg.StopReason)
	require.Equal(t, 5, msg.Usage.InputTokens)
	require.Equal(t, 7, msg.Usage.OutputTokens)

	raw, err := json.Marshal(msg)
	require.NoError(t, err)
	require.Contains(t, string(raw), `"stop_sequence":null`)
}

func TestChatToMessagesToolCallsAndReasoning(t *testing.T) {
	var resp ChatResponse
	require.NoError(t, json.Unmarshal([]byte(`{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":null,"reasoning_content":"hmm",
		"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{\"a\":1}"}},
		              {"id":"","type":"function","function":{"name":"g","arguments":"not json"}}]},"finish_reason":"tool_calls"}]}`), &resp))
	msg := ChatToMessages(&resp, "m")
	require.Equal(t, StopToolUse, msg.StopReason)
	require.Len(t, msg.Content, 3)
	require.Equal(t, "thinking", msg.Content[0].Type)
	require.Equal(t, "hmm", msg.Content[0].Thinking)
	require.Equal(t, "tool_use", msg.Content[1].Type)
	require.Equal(t, "call_1", msg.Content[1].ID)
	require.JSONEq(t, `{"a":1}`, string(msg.Content[1].Input))
	require.True(t, strings.HasPrefix(msg.Content[2].ID, "toolu_"))
	require.JSONEq(t, `{}`, string(msg.Content[2].Input), "malformed arguments degrade to an empty object")
}

func TestChatToMessagesLengthAndEmpty(t *testing.T) {
	var resp ChatResponse
	require.NoError(t, json.Unmarshal([]byte(`{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"partial"},"finish_reason":"length"}]}`), &resp))
	require.Equal(t, StopMaxTokens, ChatToMessages(&resp, "m").StopReason)

	empty := ChatToMessages(&ChatResponse{ID: ""}, "m")
	require.True(t, strings.HasPrefix(empty.ID, "msg_"))
	require.Len(t, empty.Content, 1)
	require.Equal(t, "", empty.Content[0].Text)
}

func feedJSON(t *testing.T, s *StreamState, chunk string) []Event {
	t.Helper()
	var c ChatChunk
	require.NoError(t, json.Unmarshal([]byte(chunk), &c))
	return s.Feed(&c)
}

func eventTypes(events []Event) []string {
	out := make([]string, 0, len(events))
	for _, e := range events {
		out = append(out, e.Type())
	}
	return out
}

func TestStreamTextSequence(t *testing.T) {
	s := NewStreamState("claude-alias")
	var all []Event
	all = append(all, feedJSON(t, s, `{"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c1","choices":[{"index":0,"delta":{"content":" there"},"finish_reason":"stop"}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c1","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}`)...)
	all = append(all, s.Finish()...)

	require.Equal(t, []string{
		"message_start", "content_block_start", "content_block_delta",
		"content_block_delta",
		"content_block_stop", "message_delta", "message_stop",
	}, eventTypes(all))

	start := all[0]["message"].(map[string]any)
	require.Equal(t, "claude-alias", start["model"])
	require.Equal(t, "msg_c1", start["id"])
	require.Equal(t, "text", all[1]["content_block"].(map[string]any)["type"])
	require.Equal(t, "hi", all[2]["delta"].(map[string]any)["text"])
	require.Equal(t, " there", all[3]["delta"].(map[string]any)["text"])
	require.Equal(t, StopEndTurn, all[5]["delta"].(map[string]any)["stop_reason"])
	require.Equal(t, 2, all[5]["usage"].(map[string]any)["output_tokens"])

	in, out, exact := s.Usage()
	require.True(t, exact)
	require.Equal(t, 3, in)
	require.Equal(t, 2, out)
	require.Nil(t, s.Finish(), "Finish is idempotent")
}

func TestStreamToolCallSequence(t *testing.T) {
	s := NewStreamState("m")
	var all []Event
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"content":"Let me look."},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\":"}}]},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"SH\"}"}}]},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"get_time","arguments":"{}"}}]},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}`)...)
	all = append(all, s.Finish()...)

	require.Equal(t, []string{
		"message_start",
		"content_block_start", "content_block_delta", // text
		"content_block_stop", "content_block_start", // close text, open tool 0
		"content_block_delta", "content_block_delta", // two arg fragments
		"content_block_stop", "content_block_start", "content_block_delta", // tool 1 with args
		"content_block_stop", "message_delta", "message_stop",
	}, eventTypes(all))

	tool0 := all[4]["content_block"].(map[string]any)
	require.Equal(t, "tool_use", tool0["type"])
	require.Equal(t, "call_a", tool0["id"])
	require.Equal(t, "get_weather", tool0["name"])
	require.Equal(t, 1, all[4]["index"])
	require.Equal(t, `{"city":`, all[5]["delta"].(map[string]any)["partial_json"])
	require.Equal(t, 1, all[5]["index"])
	tool1 := all[8]["content_block"].(map[string]any)
	require.Equal(t, "call_b", tool1["id"])
	require.Equal(t, 2, all[8]["index"])
	require.Equal(t, StopToolUse, all[11]["delta"].(map[string]any)["stop_reason"])
}

func TestStreamReasoningBecomesThinkingBlock(t *testing.T) {
	s := NewStreamState("m")
	var all []Event
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"reasoning_content":"think"},"finish_reason":null}]}`)...)
	all = append(all, feedJSON(t, s, `{"id":"c","choices":[{"index":0,"delta":{"content":"answer"},"finish_reason":"stop"}]}`)...)
	all = append(all, s.Finish()...)
	require.Equal(t, []string{
		"message_start",
		"content_block_start", "content_block_delta", "content_block_stop", // thinking
		"content_block_start", "content_block_delta", "content_block_stop", // text
		"message_delta", "message_stop",
	}, eventTypes(all))
	require.Equal(t, "thinking", all[1]["content_block"].(map[string]any)["type"])
	require.Equal(t, "think", all[2]["delta"].(map[string]any)["thinking"])
	require.Equal(t, "text", all[4]["content_block"].(map[string]any)["type"])
	_, out, exact := s.Usage()
	require.False(t, exact)
	require.Greater(t, out, 0, "output is estimated when the upstream sends no usage")
}

func TestStreamEmptyUpstreamStillYieldsMessage(t *testing.T) {
	s := NewStreamState("m")
	all := s.Finish()
	require.Equal(t, []string{"message_start", "message_delta", "message_stop"}, eventTypes(all))
}

func TestStreamIgnoresSecondaryChoices(t *testing.T) {
	s := NewStreamState("m")
	events := feedJSON(t, s, `{"id":"c","choices":[{"index":1,"delta":{"content":"ignored"},"finish_reason":null}]}`)
	require.Equal(t, []string{"message_start"}, eventTypes(events))
}

func TestEncodeSSE(t *testing.T) {
	frame, err := EncodeSSE(Event{"type": "message_stop"})
	require.NoError(t, err)
	require.Equal(t, "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n", frame)
	_, err = EncodeSSE(Event{"index": 1})
	require.Error(t, err)
}

func TestParseSSELine(t *testing.T) {
	payload, done, ok := ParseSSELine(`data: {"a":1}`)
	require.True(t, ok)
	require.False(t, done)
	require.Equal(t, `{"a":1}`, payload)
	_, done, ok = ParseSSELine("data: [DONE]")
	require.True(t, ok)
	require.True(t, done)
	_, _, ok = ParseSSELine("event: ping")
	require.False(t, ok)
	_, _, ok = ParseSSELine("data:")
	require.False(t, ok)
}

func TestEstimateTokens(t *testing.T) {
	require.Equal(t, 0, EstimateTokens(""))
	require.Equal(t, 1, EstimateTokens("hi"))
	require.Equal(t, 3, EstimateTokens("你好吗"))
	require.Equal(t, 2, EstimateTokens("hello wo"))
}
