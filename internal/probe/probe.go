// Package probe performs lightweight health checks against upstream model
// providers. A probe sends a tiny arithmetic question to a model and verifies
// that the answer comes back, which proves the endpoint, credentials, and
// model name all work end to end. The package has no dependency on the rest
// of HeiGate so it can be tested in isolation.
package probe

import (
	"bufio"
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// Provider identifiers. The values are persisted in the local database and
// exchanged with the desktop UI, so they must stay stable.
const (
	ProviderOpenAI    = "openai"
	ProviderAnthropic = "anthropic"
)

// API modes for OpenAI-compatible providers.
const (
	APIModeChatCompletions = "chat_completions"
	APIModeResponses       = "responses"
)

// Health statuses reported by a probe.
const (
	StatusOperational = "operational" // answered correctly within the latency budget
	StatusDegraded    = "degraded"    // answered correctly but slowly
	StatusFailed      = "failed"      // reachable, but the answer was wrong or empty
	StatusError       = "error"       // transport or HTTP failure
)

// Timing and limits.
const (
	// RequestTimeout bounds a single probe request including body read.
	RequestTimeout = 60 * time.Second
	// MinIntervalSeconds is the shortest automatic probe interval.
	MinIntervalSeconds = 15
	// MaxIntervalSeconds is the longest automatic probe interval (24h).
	MaxIntervalSeconds = 86400
	// ScheduleBuffer is extra slack added to the per-run deadline by schedulers.
	ScheduleBuffer = 10 * time.Second

	// maxAnswerTokens is deliberately tiny: the probe aborts the stream at the
	// first content delta anyway, so the cap only bounds what a provider may
	// generate before that. It is not 1 because some upstreams reject that
	// outright (B.AI: "max_tokens must be greater than 2"); 16 clears every
	// lower bound seen in the wild while keeping generation negligible.
	maxAnswerTokens  = 16
	maxErrorSnippet  = 300
	maxErrorBodyRead = 64 << 10
)

// DegradedThreshold is the latency at or above which a correct answer is
// still reported as degraded. It is a variable so tests can tighten it.
var DegradedThreshold = 6 * time.Second

// RunBudget is the wall-clock a full Run may need for one model: one attempt
// plus scheduling slack. All probe modes intentionally use one attempt; a
// later manual click or scheduler tick is the next sample.
func RunBudget() time.Duration {
	return RunBudgetFor(1)
}

// RunBudgetFor calculates the outer deadline for probing modelCount models
// sequentially in a queue, including one attempt per model and slack.
func RunBudgetFor(modelCount int) time.Duration {
	if modelCount < 1 {
		modelCount = 1
	}
	return time.Duration(modelCount)*RequestTimeout + ScheduleBuffer
}

// Validation errors. Messages intentionally contain "invalid" / "required"
// so HTTP handlers can map them to 400 responses.
var (
	ErrInvalidProvider     = errors.New("invalid provider: expected \"openai\" or \"anthropic\"")
	ErrInvalidAPIMode      = errors.New("invalid api_mode: expected \"chat_completions\" or \"responses\" (responses is OpenAI only)")
	ErrInvalidEndpoint     = errors.New("invalid endpoint: expected an http:// or https:// URL")
	ErrInvalidInterval     = errors.New("invalid interval_seconds: expected 0 (manual) or a value between 15 and 86400")
	ErrMissingPrimaryModel = errors.New("primary_model is required")
)

// Config describes one upstream channel to probe.
type Config struct {
	Provider     string
	APIMode      string
	Endpoint     string
	APIKey       string
	PrimaryModel string
	ExtraModels  []string
	ExtraHeaders map[string]string
}

// Result is the outcome of probing a single model.
type Result struct {
	Model         string    `json:"model"`
	Status        string    `json:"status"`
	LatencyMs     *int      `json:"latency_ms,omitempty"`
	TTFTMs        *int      `json:"ttft_ms,omitempty"`
	PingLatencyMs *int      `json:"ping_latency_ms,omitempty"`
	Message       string    `json:"message,omitempty"`
	CheckedAt     time.Time `json:"checked_at"`
}

var (
	requestClient = &http.Client{Timeout: RequestTimeout}
)

// NormalizeAPIMode maps an empty mode to chat_completions.
func NormalizeAPIMode(mode string) string {
	mode = strings.TrimSpace(mode)
	if mode == "" {
		return APIModeChatCompletions
	}
	return mode
}

// NormalizeEndpoint trims whitespace, adds https:// when no scheme is given,
// and removes a trailing slash and a trailing /v1 segment so both
// "https://api.example.com" and "https://api.example.com/v1/" resolve to the
// same origin-style base URL.
func NormalizeEndpoint(raw string) string {
	ep := strings.TrimSpace(raw)
	if ep == "" {
		return ""
	}
	lower := strings.ToLower(ep)
	if !strings.Contains(lower, "://") {
		ep = "https://" + ep
	}
	ep = strings.TrimRight(ep, "/")
	ep = strings.TrimSuffix(ep, "/v1")
	return strings.TrimRight(ep, "/")
}

// ValidateConfig checks provider, api mode, endpoint and primary model.
func ValidateConfig(cfg Config) error {
	provider := strings.TrimSpace(cfg.Provider)
	if provider != ProviderOpenAI && provider != ProviderAnthropic {
		return ErrInvalidProvider
	}
	switch NormalizeAPIMode(cfg.APIMode) {
	case APIModeChatCompletions:
	case APIModeResponses:
		if provider != ProviderOpenAI {
			return ErrInvalidAPIMode
		}
	default:
		return ErrInvalidAPIMode
	}
	if err := validateEndpoint(cfg.Endpoint); err != nil {
		return err
	}
	if strings.TrimSpace(cfg.PrimaryModel) == "" {
		return ErrMissingPrimaryModel
	}
	return nil
}

// ValidateInterval accepts 0 (manual probing only) or a value within the
// supported automatic range.
func ValidateInterval(seconds int) error {
	if seconds == 0 {
		return nil
	}
	if seconds < MinIntervalSeconds || seconds > MaxIntervalSeconds {
		return ErrInvalidInterval
	}
	return nil
}

func validateEndpoint(raw string) error {
	ep := NormalizeEndpoint(raw)
	if ep == "" {
		return ErrInvalidEndpoint
	}
	u, err := url.Parse(ep)
	if err != nil || u.Host == "" {
		return ErrInvalidEndpoint
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return ErrInvalidEndpoint
	}
	return nil
}

// Run probes the primary model and every extra model sequentially in a queue and
// returns one Result per distinct model, in configuration order. Requesting
// sequentially avoids triggering upstream concurrency limits or rate limits.
// A non-nil error is only returned for invalid configuration; upstream failures
// are represented as per-model results.
func Run(ctx context.Context, cfg Config) ([]*Result, error) {
	return run(ctx, cfg)
}

// RunOnce is kept as an explicit name for scheduler callers. It has the same
// single-attempt semantics as Run.
func RunOnce(ctx context.Context, cfg Config) ([]*Result, error) {
	return run(ctx, cfg)
}

func run(ctx context.Context, cfg Config) ([]*Result, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	cfg.Provider = strings.TrimSpace(cfg.Provider)
	cfg.APIMode = NormalizeAPIMode(cfg.APIMode)
	cfg.Endpoint = NormalizeEndpoint(cfg.Endpoint)
	cfg.APIKey = strings.TrimSpace(cfg.APIKey)
	cfg.PrimaryModel = strings.TrimSpace(cfg.PrimaryModel)
	if err := ValidateConfig(cfg); err != nil {
		return nil, err
	}

	models := dedupeModels(append([]string{cfg.PrimaryModel}, cfg.ExtraModels...))
	if len(models) == 0 {
		return nil, ErrMissingPrimaryModel
	}

	results := make([]*Result, len(models))
	for i, model := range models {
		if ctx.Err() != nil {
			results[i] = &Result{
				Model:     model,
				Status:    StatusError,
				Message:   ctx.Err().Error(),
				CheckedAt: time.Now(),
			}
			continue
		}
		res := checkModel(ctx, cfg, model)
		results[i] = res
	}
	return results, nil
}

type challenge struct {
	prompt   string
	expected string
}

func newChallenge() challenge {
	a := randomInt(10, 89)
	b := randomInt(10, 89)
	// A bare sum: reasoning models spend far longer thinking about an English
	// instruction ("Answer with digits only...") than about plain arithmetic
	// (measured 35s vs 8s on the same model), and answerMatches scans digit
	// runs anyway, so decorated answers still pass.
	return challenge{
		prompt:   fmt.Sprintf("%d + %d = ?", a, b),
		expected: strconv.Itoa(a + b),
	}
}

func randomInt(lo, hi int) int {
	n, err := rand.Int(rand.Reader, big.NewInt(int64(hi-lo+1)))
	if err != nil {
		return lo + int(time.Now().UnixNano()%int64(hi-lo+1))
	}
	return lo + int(n.Int64())
}

var digitRun = regexp.MustCompile(`\d+`)

// answerMatches reports whether the expected number appears as a standalone
// digit run in the reply. Models often decorate the answer ("42.", "**42**",
// "The answer is 42"), so we scan every digit run rather than the whole text.
func answerMatches(reply, expected string) bool {
	for _, run := range digitRun.FindAllString(reply, -1) {
		if strings.TrimLeft(run, "0") == expected || run == expected {
			return true
		}
	}
	return false
}

func checkModel(ctx context.Context, cfg Config, model string) *Result {
	return checkModelOnce(ctx, cfg, model, RequestTimeout)
}

// checkModelOnce performs a single challenge request.
func checkModelOnce(ctx context.Context, cfg Config, model string, attemptTimeout time.Duration) *Result {
	res := &Result{Model: model, Status: StatusError, CheckedAt: time.Now()}
	ch := newChallenge()

	reqCtx, cancel := context.WithTimeout(ctx, attemptTimeout)
	defer cancel()

	started := time.Now()
	out, err := send(reqCtx, cfg, model, ch.prompt)
	latency := time.Since(started)
	latencyMs := int(latency / time.Millisecond)
	if err == nil && out.hasTTFT {
		ttftMs := int(out.ttft / time.Millisecond)
		res.TTFTMs = &ttftMs
		// Probes answer one question: when did the model first become usable?
		// Expose that same first-token measurement as LatencyMs so every UI
		// surface reports one consistent probe time instead of mixing TTFT and
		// post-token stream duration.
		latencyMs = ttftMs
	}
	res.LatencyMs = &latencyMs

	switch {
	case err != nil:
		res.Message = redact(truncate(err.Error(), maxErrorSnippet), cfg.APIKey)
	case out.status >= 500:
		// Preserve the upstream status in the result; the caller will decide when
		// to take the next independent sample.
		res.Message = redact(fmt.Sprintf("upstream HTTP %d: %s", out.status, truncate(compactWhitespace(out.body), maxErrorSnippet)), cfg.APIKey)
	case out.status < 200 || out.status >= 300:
		res.Message = redact(fmt.Sprintf("upstream HTTP %d: %s", out.status, truncate(compactWhitespace(out.body), maxErrorSnippet)), cfg.APIKey)
	case !out.sawContent && !out.sawReasoning:
		// 2xx but nothing was ever produced: the channel answers HTTP yet cannot
		// serve a request, which is the failure worth catching. Reasoning-only
		// output does not land here — with generation capped at maxAnswerTokens
		// a slow thinker legitimately spends the whole budget before its first
		// visible token, and it still proved the whole path works.
		res.Status = StatusFailed
		res.Message = "upstream returned 2xx but no content"
	case time.Duration(latencyMs)*time.Millisecond >= DegradedThreshold:
		res.Status = StatusDegraded
		res.Message = fmt.Sprintf("slow response: %dms", latencyMs)
	default:
		res.Status = StatusOperational
	}
	// Answer correctness is a soft signal, not a verdict: the probe caps
	// generation at maxAnswerTokens and stops at the first content delta, so a
	// correct answer is usually truncated ("8" of "85"). A channel that streams
	// content is usable for real traffic even when this note fires.
	if res.Status == StatusOperational || res.Status == StatusDegraded {
		if !answerMatches(out.reply, ch.expected) && res.Message == "" {
			res.Message = truncate(fmt.Sprintf("answer not verified (wanted %s, got %q)", ch.expected, compactWhitespace(out.reply)), maxErrorSnippet)
		}
	}
	return res
}

// sendResult carries what a probe request learned. body holds the raw error
// payload and is only meaningful for non-2xx statuses.
type sendResult struct {
	reply        string
	ttft         time.Duration
	hasTTFT      bool
	sawContent   bool
	sawReasoning bool
	sawAnyDelta  bool
	status       int
	body         string
}

// maxStreamBytes bounds how much of a streamed answer is read. The challenge
// answer is tiny; the cap only stops a broken upstream from streaming forever
// into memory.
const maxStreamBytes = 1 << 20

// send performs one streaming provider request. The first visible text delta
// is recorded as the time to first token — the number a streaming client
// actually waits for — falling back to the first reasoning delta for models
// that answer entirely inside thinking. Total time (stream end) is measured
// by the caller.
func send(ctx context.Context, cfg Config, model, prompt string) (*sendResult, error) {
	var path string
	var payload any
	headers := map[string]string{"Content-Type": "application/json", "Accept": "application/json"}

	switch cfg.Provider {
	case ProviderAnthropic:
		path = "/v1/messages"
		payload = map[string]any{
			"model":      model,
			"max_tokens": maxAnswerTokens,
			"messages":   []map[string]any{{"role": "user", "content": prompt}},
			"stream":     true,
		}
		if cfg.APIKey != "" {
			headers["x-api-key"] = cfg.APIKey
		}
		headers["anthropic-version"] = "2023-06-01"
	case ProviderOpenAI:
		if cfg.APIKey != "" {
			headers["Authorization"] = "Bearer " + cfg.APIKey
		}
		if cfg.APIMode == APIModeResponses {
			path = "/v1/responses"
			payload = map[string]any{
				"model":             model,
				"input":             prompt,
				"max_output_tokens": maxAnswerTokens,
				"stream":            true,
			}
		} else {
			path = "/v1/chat/completions"
			// No temperature: some models only accept a single value (kimi-k3
			// rejects anything but 1), and determinism does not matter for a
			// two-digit addition.
			payload = map[string]any{
				"model":      model,
				"messages":   []map[string]any{{"role": "user", "content": prompt}},
				"max_tokens": maxAnswerTokens,
				"stream":     true,
			}
		}
	default:
		return nil, ErrInvalidProvider
	}
	for k, v := range cfg.ExtraHeaders {
		if strings.TrimSpace(k) != "" {
			headers[k] = v
		}
	}

	raw, err := json.Marshal(payload)
	if err != nil {
		return nil, fmt.Errorf("encode probe request: %w", err)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, cfg.Endpoint+path, bytes.NewReader(raw))
	if err != nil {
		return nil, err
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	started := time.Now()
	resp, err := requestClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		data, _ := io.ReadAll(io.LimitReader(resp.Body, maxErrorBodyRead))
		return &sendResult{status: resp.StatusCode, body: string(data)}, nil
	}
	return readStream(cfg, resp.Body, started)
}

// readStream consumes the SSE stream and accumulates the answer. text and
// reasoning are tracked separately so the TTFT can prefer visible text.
func readStream(cfg Config, body io.Reader, started time.Time) (*sendResult, error) {
	out := &sendResult{status: http.StatusOK}
	var text, reasoning strings.Builder
	var textAt, reasonAt time.Time

	scanner := bufio.NewScanner(io.LimitReader(body, maxStreamBytes))
	scanner.Buffer(make([]byte, 0, 64*1024), 256*1024)
	for scanner.Scan() {
		payload, done, ok := parseSSELine(scanner.Text())
		if !ok {
			continue
		}
		if done {
			break
		}
		out.sawAnyDelta = true
		// Upstream providers may emit an error object inside the SSE stream (e.g. 503 or quota error with 200 HTTP status)
		var errPayload struct {
			Error json.RawMessage `json:"error"`
			Type  string          `json:"type"`
		}
		if json.Unmarshal([]byte(payload), &errPayload) == nil && len(errPayload.Error) > 0 {
			var errMsg string
			var obj struct {
				Message string `json:"message"`
			}
			if json.Unmarshal(errPayload.Error, &obj) == nil && obj.Message != "" {
				errMsg = obj.Message
			} else if json.Unmarshal(errPayload.Error, &errMsg) != nil || errMsg == "" {
				errMsg = string(errPayload.Error)
			}
			return nil, fmt.Errorf("upstream stream error: %s", compactWhitespace(errMsg))
		}

		switch {
		case cfg.Provider == ProviderAnthropic:
			var ev struct {
				Type  string `json:"type"`
				Delta struct {
					Type     string `json:"type"`
					Text     string `json:"text"`
					Thinking string `json:"thinking"`
				} `json:"delta"`
			}
			if json.Unmarshal([]byte(payload), &ev) != nil || ev.Type != "content_block_delta" {
				continue
			}
			switch ev.Delta.Type {
			case "text_delta":
				if ev.Delta.Text != "" {
					if textAt.IsZero() {
						textAt = time.Now()
					}
					text.WriteString(ev.Delta.Text)
				}
			case "thinking_delta":
				if ev.Delta.Thinking != "" {
					if reasonAt.IsZero() {
						reasonAt = time.Now()
					}
					reasoning.WriteString(ev.Delta.Thinking)
				}
			}
		case cfg.APIMode == APIModeResponses:
			var ev struct {
				Type  string `json:"type"`
				Delta string `json:"delta"`
			}
			if json.Unmarshal([]byte(payload), &ev) != nil {
				continue
			}
			switch ev.Type {
			case "response.output_text.delta":
				if ev.Delta != "" {
					if textAt.IsZero() {
						textAt = time.Now()
					}
					text.WriteString(ev.Delta)
				}
			case "response.reasoning_text.delta":
				if ev.Delta != "" {
					if reasonAt.IsZero() {
						reasonAt = time.Now()
					}
					reasoning.WriteString(ev.Delta)
				}
			}
		default:
			var ev struct {
				Choices []struct {
					Delta struct {
						Content          json.RawMessage `json:"content"`
						ReasoningContent string          `json:"reasoning_content"`
						Reasoning        string          `json:"reasoning"`
						Thinking         string          `json:"thinking"`
						Thought          string          `json:"thought"`
					} `json:"delta"`
				} `json:"choices"`
			}
			if json.Unmarshal([]byte(payload), &ev) != nil {
				continue
			}
			for _, choice := range ev.Choices {
				if part := contentToText(choice.Delta.Content); part != "" {
					if textAt.IsZero() {
						textAt = time.Now()
					}
					text.WriteString(part)
				}
				thinking := choice.Delta.ReasoningContent
				if thinking == "" {
					thinking = choice.Delta.Reasoning
				}
				if thinking == "" {
					thinking = choice.Delta.Thinking
				}
				if thinking == "" {
					thinking = choice.Delta.Thought
				}
				if thinking != "" {
					if reasonAt.IsZero() {
						reasonAt = time.Now()
					}
					reasoning.WriteString(thinking)
				}
			}
		}
		// Everything a probe needs — endpoint, credentials, model name, quota,
		// the fact that content is actually produced, and the time to first
		// token — is proven by the first content delta. Stop reading there and
		// let the caller close the body: the rest of the stream costs upstream
		// tokens and, on a slow reasoning model, tens of seconds of waiting.
		if !textAt.IsZero() {
			break
		}
	}
	if err := scanner.Err(); err != nil {
		return nil, fmt.Errorf("read probe stream: %w", err)
	}

	out.sawContent = !textAt.IsZero()
	out.sawReasoning = !reasonAt.IsZero()
	out.reply = strings.TrimSpace(text.String())
	if out.reply == "" {
		out.reply = strings.TrimSpace(reasoning.String())
	}
	// Prefer the first visible text token; fall back to reasoning for models
	// that only answer inside thinking.
	if !textAt.IsZero() {
		out.ttft = textAt.Sub(started)
		out.hasTTFT = true
	} else if !reasonAt.IsZero() {
		out.ttft = reasonAt.Sub(started)
		out.hasTTFT = true
	}
	return out, nil
}

// parseSSELine extracts the JSON payload from a "data: ..." line. done is
// true for the OpenAI "[DONE]" sentinel; ok is false for non-data lines.
func parseSSELine(line string) (payload string, done bool, ok bool) {
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

// contentToText handles both string content and an array of content parts.
func contentToText(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return s
	}
	var parts []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if json.Unmarshal(raw, &parts) == nil {
		var sb strings.Builder
		for _, p := range parts {
			if p.Type == "" || p.Type == "text" || p.Type == "output_text" {
				sb.WriteString(p.Text)
			}
		}
		return sb.String()
	}
	return ""
}

func dedupeModels(in []string) []string {
	seen := make(map[string]struct{}, len(in))
	out := make([]string, 0, len(in))
	for _, m := range in {
		m = strings.TrimSpace(m)
		if m == "" {
			continue
		}
		if _, ok := seen[m]; ok {
			continue
		}
		seen[m] = struct{}{}
		out = append(out, m)
	}
	return out
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + "…"
}

func compactWhitespace(s string) string {
	return strings.Join(strings.Fields(s), " ")
}

// redact hides the API key if a provider echoes it back in an error body.
func redact(s, secret string) string {
	if secret == "" || len(secret) < 6 {
		return s
	}
	return strings.ReplaceAll(s, secret, "***")
}
