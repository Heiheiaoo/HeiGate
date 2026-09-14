// Package probe performs lightweight health checks against upstream model
// providers. A probe sends a tiny arithmetic question to a model and verifies
// that the answer comes back, which proves the endpoint, credentials, and
// model name all work end to end. The package has no dependency on the rest
// of HeiGate so it can be tested in isolation.
package probe

import (
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
	"sync"
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
	RequestTimeout = 45 * time.Second
	// PingTimeout bounds the HEAD request used to measure origin reachability.
	PingTimeout = 8 * time.Second
	// MinIntervalSeconds is the shortest automatic probe interval.
	MinIntervalSeconds = 15
	// MaxIntervalSeconds is the longest automatic probe interval (24h).
	MaxIntervalSeconds = 86400
	// ScheduleBuffer is extra slack added to the per-run deadline by schedulers.
	ScheduleBuffer = 10 * time.Second

	maxAnswerTokens  = 64
	maxErrorSnippet  = 300
	maxErrorBodyRead = 64 << 10
)

// DegradedThreshold is the latency at or above which a correct answer is
// still reported as degraded. It is a variable so tests can tighten it.
var DegradedThreshold = 6 * time.Second

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
	PingLatencyMs *int      `json:"ping_latency_ms,omitempty"`
	Message       string    `json:"message,omitempty"`
	CheckedAt     time.Time `json:"checked_at"`
}

var (
	requestClient = &http.Client{Timeout: RequestTimeout}
	pingClient    = &http.Client{
		Timeout: PingTimeout,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
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

// Run probes the primary model and every extra model concurrently and
// returns one Result per distinct model, in configuration order. The origin
// ping latency is measured once and attached to every result. A non-nil
// error is only returned for invalid configuration.
func Run(ctx context.Context, cfg Config) ([]*Result, error) {
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

	ping := Ping(ctx, cfg.Endpoint)
	results := make([]*Result, len(models))
	var wg sync.WaitGroup
	for i, model := range models {
		wg.Add(1)
		go func(i int, model string) {
			defer wg.Done()
			res := checkModel(ctx, cfg, model)
			res.PingLatencyMs = ping
			results[i] = res
		}(i, model)
	}
	wg.Wait()
	return results, nil
}

// Ping measures a HEAD round trip to the endpoint origin. It returns nil when
// the origin cannot be reached; any HTTP status counts as reachable.
func Ping(ctx context.Context, endpoint string) *int {
	u, err := url.Parse(NormalizeEndpoint(endpoint))
	if err != nil || u.Host == "" {
		return nil
	}
	origin := u.Scheme + "://" + u.Host + "/"
	req, err := http.NewRequestWithContext(ctx, http.MethodHead, origin, nil)
	if err != nil {
		return nil
	}
	started := time.Now()
	resp, err := pingClient.Do(req)
	if err != nil {
		return nil
	}
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
	_ = resp.Body.Close()
	ms := int(time.Since(started) / time.Millisecond)
	return &ms
}

type challenge struct {
	prompt   string
	expected string
}

func newChallenge() challenge {
	a := randomInt(10, 89)
	b := randomInt(10, 89)
	return challenge{
		prompt:   fmt.Sprintf("Answer with digits only, no words: %d + %d = ?", a, b),
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
	res := &Result{Model: model, Status: StatusError, CheckedAt: time.Now()}
	ch := newChallenge()

	reqCtx, cancel := context.WithTimeout(ctx, RequestTimeout)
	defer cancel()

	started := time.Now()
	reply, status, body, err := send(reqCtx, cfg, model, ch.prompt)
	latency := time.Since(started)
	latencyMs := int(latency / time.Millisecond)
	res.LatencyMs = &latencyMs

	switch {
	case err != nil:
		res.Message = redact(truncate(err.Error(), maxErrorSnippet), cfg.APIKey)
	case status < 200 || status >= 300:
		res.Message = redact(fmt.Sprintf("upstream HTTP %d: %s", status, truncate(compactWhitespace(body), maxErrorSnippet)), cfg.APIKey)
	case !answerMatches(reply, ch.expected):
		res.Status = StatusFailed
		if strings.TrimSpace(reply) == "" {
			res.Message = "upstream returned 2xx but no text content"
		} else {
			res.Message = truncate(fmt.Sprintf("unexpected answer: wanted %s, got %q", ch.expected, compactWhitespace(reply)), maxErrorSnippet)
		}
	case latency >= DegradedThreshold:
		res.Status = StatusDegraded
		res.Message = fmt.Sprintf("slow response: %dms", latencyMs)
	default:
		res.Status = StatusOperational
	}
	return res
}

// send performs one provider request and returns the reply text, HTTP status
// and raw body (for error reporting).
func send(ctx context.Context, cfg Config, model, prompt string) (reply string, status int, body string, err error) {
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
				"stream":            false,
			}
		} else {
			path = "/v1/chat/completions"
			payload = map[string]any{
				"model":       model,
				"messages":    []map[string]any{{"role": "user", "content": prompt}},
				"max_tokens":  maxAnswerTokens,
				"temperature": 0,
				"stream":      false,
			}
		}
	default:
		return "", 0, "", ErrInvalidProvider
	}
	for k, v := range cfg.ExtraHeaders {
		if strings.TrimSpace(k) != "" {
			headers[k] = v
		}
	}

	raw, err := json.Marshal(payload)
	if err != nil {
		return "", 0, "", fmt.Errorf("encode probe request: %w", err)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, cfg.Endpoint+path, bytes.NewReader(raw))
	if err != nil {
		return "", 0, "", err
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := requestClient.Do(req)
	if err != nil {
		return "", 0, "", err
	}
	defer func() { _ = resp.Body.Close() }()
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxErrorBodyRead))
	if err != nil {
		return "", resp.StatusCode, "", fmt.Errorf("read probe response: %w", err)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", resp.StatusCode, string(data), nil
	}
	return extractText(cfg, data), resp.StatusCode, string(data), nil
}

// extractText pulls assistant text out of the three supported response
// shapes. Reasoning text is included as a fallback so "thinking" models that
// spend their token budget before emitting content still get a fair check.
func extractText(cfg Config, data []byte) string {
	var sb strings.Builder
	switch {
	case cfg.Provider == ProviderAnthropic:
		var msg struct {
			Content []struct {
				Type string `json:"type"`
				Text string `json:"text"`
			} `json:"content"`
		}
		if json.Unmarshal(data, &msg) == nil {
			for _, block := range msg.Content {
				if block.Type == "text" {
					sb.WriteString(block.Text)
					sb.WriteByte(' ')
				}
			}
		}
	case cfg.APIMode == APIModeResponses:
		var out struct {
			OutputText string `json:"output_text"`
			Output     []struct {
				Type    string `json:"type"`
				Content []struct {
					Type string `json:"type"`
					Text string `json:"text"`
				} `json:"content"`
			} `json:"output"`
		}
		if json.Unmarshal(data, &out) == nil {
			sb.WriteString(out.OutputText)
			for _, item := range out.Output {
				for _, part := range item.Content {
					if part.Type == "output_text" || part.Type == "text" || part.Type == "reasoning_text" {
						sb.WriteByte(' ')
						sb.WriteString(part.Text)
					}
				}
			}
		}
	default:
		var out struct {
			Choices []struct {
				Message struct {
					Content          json.RawMessage `json:"content"`
					ReasoningContent string          `json:"reasoning_content"`
				} `json:"message"`
			} `json:"choices"`
		}
		if json.Unmarshal(data, &out) == nil {
			for _, choice := range out.Choices {
				sb.WriteString(contentToText(choice.Message.Content))
				if sb.Len() == 0 {
					sb.WriteString(choice.Message.ReasoningContent)
				}
				sb.WriteByte(' ')
			}
		}
	}
	return strings.TrimSpace(sb.String())
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
