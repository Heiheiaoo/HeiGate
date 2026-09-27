package api

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Heiheiaoo/heigate/internal/channel"
	"github.com/Heiheiaoo/heigate/internal/probe"
	"github.com/Heiheiaoo/heigate/internal/protocol"
	"github.com/gin-gonic/gin"
)

const maxRequestBody = 16 << 20

// upstreamClient is shared so keep-alive connections are reused across
// requests. The generous timeout covers long streaming completions.
var upstreamClient = &http.Client{Timeout: 10 * time.Minute}

type upstreamResponse struct {
	status int
	header http.Header
	body   io.ReadCloser
}

// RegisterGatewayRoutes mounts protocol-compatible local gateway endpoints.
// The group must be protected with LoopbackOnly by the caller.
func (h *Handler) RegisterGatewayRoutes(group *gin.RouterGroup) {
	group.POST("/v1/messages", h.forwardMessages)
	group.POST("/v1/chat/completions", h.forwardChatCompletions)
	group.POST("/v1/responses", h.forwardResponses)
	group.GET("/v1/models", h.gatewayModels)
}

func (h *Handler) forwardMessages(c *gin.Context) {
	h.forward(c, protocolAnthropic)
}

func (h *Handler) forwardChatCompletions(c *gin.Context) {
	h.forward(c, protocolChat)
}

func (h *Handler) forwardResponses(c *gin.Context) {
	h.forward(c, protocolResponses)
}

type gatewayProtocol string

const (
	protocolAnthropic gatewayProtocol = "anthropic"
	protocolChat      gatewayProtocol = "chat_completions"
	protocolResponses gatewayProtocol = "responses"
)

const autoModelName = "Auto"

func channelSupportsModel(ch *channel.Channel, model string) bool {
	if strings.EqualFold(ch.PrimaryModel, model) {
		return true
	}
	for _, m := range ch.ExtraModels {
		if strings.EqualFold(m, model) {
			return true
		}
	}
	return false
}

func (h *Handler) resolveModelAlias(ctx context.Context, requestedModel string) string {
	requestedModel = strings.TrimSpace(requestedModel)
	if requestedModel == "" {
		return ""
	}
	// 1. Check custom slot mappings from setting
	if h.store != nil {
		if mappingStr, err := h.store.GetSetting(ctx, "claude_desktop_model_mappings", ""); err == nil && mappingStr != "" {
			var mappings map[string]string
			if json.Unmarshal([]byte(mappingStr), &mappings) == nil {
				if target, ok := mappings[strings.ToLower(requestedModel)]; ok && strings.TrimSpace(target) != "" {
					return strings.TrimSpace(target)
				}
			}
		}
	}
	// 2. Check if channels directly support this model
	if h.store != nil {
		if channels, err := h.store.List(ctx); err == nil {
			for _, ch := range channels {
				if ch.Enabled && channelSupportsModel(ch, requestedModel) {
					return requestedModel
				}
			}
			// 3. Fallback: if requestedModel starts with "claude-" but no channel
			// has it, use the primary model of the healthiest enabled channel.
			// Preferring a channel that is actually serving traffic matters: the
			// previous "first enabled" rule could hand a working request to a
			// channel whose circuit is open or whose last check failed.
			if strings.HasPrefix(strings.ToLower(requestedModel), "claude-") || strings.HasPrefix(strings.ToLower(requestedModel), "anthropic/claude-") {
				var benched string
				for _, ch := range channels {
					model := strings.TrimSpace(ch.PrimaryModel)
					if !ch.Enabled || model == "" {
						continue
					}
					if channelServingTraffic(ch) {
						return model
					}
					if benched == "" {
						benched = model
					}
				}
				// Every channel looks unhealthy: better to try one than to fail
				// the request outright.
				if benched != "" {
					return benched
				}
			}
		}
	}
	return requestedModel
}

// bestAutoModel selects the best configured upstream model for one already
// eligible channel. Model-level probe state wins over the channel headline so
// an operational extra model can be used even when the primary is slow or
// unavailable. Unknown models remain eligible as a last resort.
type autoModelChoice struct {
	name  string
	rank  int
	lat   int
	order int
}

func bestAutoChoice(ch *channel.Channel) autoModelChoice {
	if ch == nil {
		return autoModelChoice{rank: 99, lat: int(^uint(0) >> 1)}
	}
	models := append([]string{ch.PrimaryModel}, ch.ExtraModels...)
	best := autoModelChoice{rank: 99, lat: int(^uint(0) >> 1), order: len(models) + 1}
	for index, name := range models {
		name = strings.TrimSpace(name)
		if name == "" {
			continue
		}
		current := autoModelChoice{name: name, rank: 2, lat: int(^uint(0) >> 1), order: index}
		if strings.EqualFold(name, ch.PrimaryModel) {
			if ch.LastStatus == probe.StatusOperational || ch.LastStatus == "healthy" {
				current.rank = 0
			} else if ch.LastStatus == probe.StatusDegraded {
				current.rank = 1
			} else if ch.LastStatus == probe.StatusError || ch.LastStatus == probe.StatusFailed {
				current.rank = 3
			}
			if ch.LastTTFTMs != nil && *ch.LastTTFTMs > 0 {
				current.lat = *ch.LastTTFTMs
			} else if ch.LastLatencyMs != nil && *ch.LastLatencyMs > 0 {
				current.lat = *ch.LastLatencyMs
			}
		}
		for _, health := range ch.ModelHealth {
			if !strings.EqualFold(strings.TrimSpace(health.Model), name) {
				continue
			}
			switch health.Status {
			case probe.StatusOperational, "healthy":
				current.rank = 0
			case probe.StatusDegraded:
				current.rank = 1
			case probe.StatusError, probe.StatusFailed:
				current.rank = 3
			default:
				current.rank = 2
			}
			if health.TTFTMs != nil && *health.TTFTMs > 0 {
				current.lat = *health.TTFTMs
			} else if health.LatencyMs != nil && *health.LatencyMs > 0 {
				current.lat = *health.LatencyMs
			}
			break
		}
		if current.rank < best.rank || (current.rank == best.rank && current.lat < best.lat) || (current.rank == best.rank && current.lat == best.lat && current.order < best.order) {
			best = current
		}
	}
	return best
}

func bestAutoModel(ch *channel.Channel) string {
	return bestAutoChoice(ch).name
}

// channelServingTraffic reports whether a channel is a sensible target right
// now: enabled, not benched by an open circuit or an active cooldown, and not
// left in a failed state by its last check. A never-probed channel counts as
// usable rather than unknown-bad.
func channelServingTraffic(ch *channel.Channel) bool {
	if ch.CircuitState == channel.CircuitOpen {
		return false
	}
	if ch.CooldownUntil != nil && time.Now().Before(*ch.CooldownUntil) {
		return false
	}
	switch ch.LastStatus {
	case probe.StatusOperational, probe.StatusDegraded, "healthy", "", "unknown":
		return true
	}
	return false
}

// TraceStep records a single attempt in the routing and failover process.
type TraceStep struct {
	Index       int    `json:"index"`
	ChannelName string `json:"channel_name"`
	ChannelID   int64  `json:"channel_id"`
	Endpoint    string `json:"endpoint"`
	Model       string `json:"model"`
	Attempt     int    `json:"attempt"`
	StatusCode  int    `json:"status_code"`
	LatencyMs   int    `json:"latency_ms"`
	TtftMs      int    `json:"ttft_ms,omitempty"`
	Error       string `json:"error,omitempty"`
	RawError    string `json:"raw_error,omitempty"`
	Succeeded   bool   `json:"succeeded"`
}

func parseUpstreamErrorMessage(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ""
	}
	var obj map[string]any
	if err := json.Unmarshal([]byte(raw), &obj); err == nil {
		if errObj, ok := obj["error"].(map[string]any); ok {
			if msg, ok := errObj["message"].(string); ok && strings.TrimSpace(msg) != "" {
				return strings.TrimSpace(msg)
			}
		}
		if msg, ok := obj["message"].(string); ok && strings.TrimSpace(msg) != "" {
			return strings.TrimSpace(msg)
		}
		if msg, ok := obj["msg"].(string); ok && strings.TrimSpace(msg) != "" {
			return strings.TrimSpace(msg)
		}
	}
	if len(raw) > 200 {
		return raw[:200] + "..."
	}
	return raw
}

// adaptUnsupportedReasoningEffort adjusts only an explicit provider
// compatibility error. Different OpenAI-compatible upstreams use different
// reasoning effort vocabularies, so a gateway request cannot safely rewrite
// high before it knows what the selected provider accepts.
func adaptUnsupportedReasoningEffort(body []byte, message string) ([]byte, bool) {
	supported := supportedReasoningEfforts(message)
	if len(supported) == 0 {
		return nil, false
	}
	var payload map[string]any
	if err := json.Unmarshal(body, &payload); err != nil || payload == nil {
		return nil, false
	}
	changed := false
	if effort, ok := payload["reasoning_effort"].(string); ok {
		if replacement := compatibleReasoningEffort(effort, supported); replacement != "" && replacement != effort {
			payload["reasoning_effort"] = replacement
			changed = true
		}
	}
	if reasoning, ok := payload["reasoning"].(map[string]any); ok {
		if effort, ok := reasoning["effort"].(string); ok {
			if replacement := compatibleReasoningEffort(effort, supported); replacement != "" && replacement != effort {
				reasoning["effort"] = replacement
				changed = true
			}
		}
	}
	if !changed {
		return nil, false
	}
	adapted, err := json.Marshal(payload)
	if err != nil {
		return nil, false
	}
	return adapted, true
}

func supportedReasoningEfforts(message string) []string {
	lower := strings.ToLower(message)
	marker := strings.Index(lower, "supported types")
	if marker < 0 {
		marker = strings.Index(lower, "supported values")
	}
	if marker < 0 {
		return nil
	}
	text := lower[marker:]
	known := []string{"xhigh", "high", "medium", "low", "minimal"}
	seen := make(map[string]struct{}, len(known))
	for _, field := range strings.Fields(text) {
		field = strings.Trim(field, ".,;:()[]{}\"'")
		if _, ok := seen[field]; ok {
			continue
		}
		for _, candidate := range known {
			if field == candidate {
				seen[field] = struct{}{}
				break
			}
		}
	}
	result := make([]string, 0, len(seen))
	for _, candidate := range known {
		if _, ok := seen[candidate]; ok {
			result = append(result, candidate)
		}
	}
	return result
}

func compatibleReasoningEffort(requested string, supported []string) string {
	requested = strings.ToLower(strings.TrimSpace(requested))
	for _, effort := range supported {
		if effort == requested {
			return requested
		}
	}
	preferences := map[string][]string{
		"high":    {"xhigh", "medium", "low"},
		"xhigh":   {"medium", "high", "low"},
		"medium":  {"high", "xhigh", "low"},
		"low":     {"medium", "high", "xhigh"},
		"minimal": {"low", "medium", "high", "xhigh"},
	}
	for _, candidate := range preferences[requested] {
		for _, effort := range supported {
			if effort == candidate {
				return candidate
			}
		}
	}
	return ""
}

func upstreamErrorPayload(raw []byte) (string, int, bool) {
	var envelope map[string]json.RawMessage
	if err := json.Unmarshal(raw, &envelope); err != nil {
		return "", 0, false
	}
	errorValue, exists := envelope["error"]
	if !exists || len(bytes.TrimSpace(errorValue)) == 0 || bytes.Equal(bytes.TrimSpace(errorValue), []byte("null")) {
		return "", 0, false
	}
	message := parseUpstreamErrorMessage(string(raw))
	if message == "" {
		message = string(errorValue)
	}
	status := http.StatusBadGateway
	var detail struct {
		Type    string `json:"type"`
		Code    string `json:"code"`
		Message string `json:"message"`
	}
	_ = json.Unmarshal(errorValue, &detail)
	classification := strings.ToLower(detail.Type + " " + detail.Code + " " + message)
	switch {
	case strings.Contains(classification, "rate_limit"), strings.Contains(classification, "rate limit"), strings.Contains(classification, "too_many_requests"), strings.Contains(classification, "too many requests"):
		status = http.StatusTooManyRequests
	case strings.Contains(classification, "authentication"), strings.Contains(classification, "unauthorized"), strings.Contains(classification, "invalid_api_key"):
		status = http.StatusUnauthorized
	case strings.Contains(classification, "permission"), strings.Contains(classification, "forbidden"):
		status = http.StatusForbidden
	case strings.Contains(classification, "model_not_found"), strings.Contains(classification, "unknown model"):
		status = http.StatusNotFound
	}
	return message, status, true
}

func isClientClosedConn(err error) bool {
	if err == nil {
		return false
	}
	if errors.Is(err, context.Canceled) {
		return true
	}
	s := strings.ToLower(err.Error())
	return strings.Contains(s, "broken pipe") ||
		strings.Contains(s, "connection reset") ||
		strings.Contains(s, "client closed") ||
		strings.Contains(s, "context canceled")
}

func (h *Handler) forward(c *gin.Context, proto gatewayProtocol) {
	body, err := io.ReadAll(io.LimitReader(c.Request.Body, maxRequestBody+1))
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "read request body: " + err.Error()})
		return
	}
	if len(body) > maxRequestBody {
		c.JSON(http.StatusRequestEntityTooLarge, gin.H{"error": "request body is too large"})
		return
	}
	model, err := requestModel(body)
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	isAuto := strings.EqualFold(model, autoModelName)
	effectiveModel := h.resolveModelAlias(c.Request.Context(), model)
	if isAuto {
		effectiveModel = autoModelName
	}
	strategy := h.routeStrategy(c)
	candidateModel := effectiveModel
	if isAuto {
		candidateModel = ""
	}
	candidates, err := h.router.CandidatesMatching(c.Request.Context(), candidateModel, strategy, gatewayCandidateMatcher(proto))
	if err != nil {
		// Never silently replace a client's requested model with an unrelated
		// healthy model. Explicit aliases are resolved above; everything else
		// must fail clearly so callers can decide how to recover.
	}
	if err != nil {
		if proto == protocolAnthropic {
			c.JSON(http.StatusServiceUnavailable, gin.H{
				"type": "error",
				"error": gin.H{
					"type":    "api_error",
					"message": fmt.Sprintf("No channel found for model %q (resolved: %q): %s", model, effectiveModel, err.Error()),
				},
			})
			return
		}
		c.JSON(http.StatusServiceUnavailable, gin.H{"error": err.Error()})
		return
	}
	if isAuto {
		sort.SliceStable(candidates, func(i, j int) bool {
			a, b := bestAutoChoice(candidates[i]), bestAutoChoice(candidates[j])
			if a.rank != b.rank {
				return a.rank < b.rank
			}
			if a.lat != b.lat {
				return a.lat < b.lat
			}
			return candidates[i].Name < candidates[j].Name
		})
	}
	var upstream upstreamResponse
	var winningChannel *channel.Channel
	var failedAttempts []string
	var traceSteps []TraceStep
	startOverall := time.Now()
	var winningTtft int
	initialPromptTokens := estimatePromptTokens(body)
	isStreamRequest := bytes.Contains(body, []byte(`"stream":true`)) || bytes.Contains(body, []byte(`"stream": true`)) || strings.Contains(strings.ToLower(c.Request.Header.Get("Accept")), "text/event-stream")

	clientIP := c.ClientIP()
	userAgent := c.Request.UserAgent()
	requestPath := c.Request.URL.Path

	var requestLog *channel.RequestLog
	if h.store != nil {
		requestLog = &channel.RequestLog{
			Model:            effectiveModel,
			ChannelName:      "路由中...",
			StatusCode:       0, // 0 = 传输中
			LatencyMs:        0,
			TtftMs:           0,
			PromptTokens:     initialPromptTokens,
			CompletionTokens: 0,
			TotalTokens:      initialPromptTokens,
			IsStream:         isStreamRequest,
			ClientIP:         clientIP,
			UserAgent:        userAgent,
			RequestPath:      requestPath,
			CreatedAt:        time.Now().UTC(),
		}
		_ = h.store.RecordLog(context.Background(), requestLog)
	}

	winChan, routeErr := h.router.RouteCandidates(c.Request.Context(), effectiveModel, candidates, func(ctx context.Context, ch *channel.Channel) error {
		upstreamModel := effectiveModel
		if isAuto {
			upstreamModel = bestAutoModel(ch)
		} else if actual, _, ok := channel.FindChannelModelForRequest(ch, effectiveModel); ok && actual != "" {
			upstreamModel = actual
		}
		if upstreamModel == "" {
			return fmt.Errorf("channel %q has no usable model", ch.Name)
		}
		var lastCandidateErr error
		retryBody := body
		for attempt := 0; attempt <= 1; attempt++ {
			if attempt > 0 {
				if ctx.Err() != nil {
					return ctx.Err()
				}
				time.Sleep(100 * time.Millisecond)
			}
			attemptStart := time.Now()
			response, requestErr := forwardUpstream(ctx, c.Request, retryBody, ch, proto, effectiveModel, upstreamModel)
			attemptLatency := int(time.Since(attemptStart) / time.Millisecond)
			step := TraceStep{
				Index:       len(traceSteps) + 1,
				ChannelName: ch.Name,
				ChannelID:   ch.ID,
				Endpoint:    ch.Endpoint,
				Model:       upstreamModel,
				Attempt:     attempt + 1,
				LatencyMs:   attemptLatency,
			}

			if requestErr != nil {
				wrapped := &transportError{err: requestErr}
				lastCandidateErr = wrapped
				step.Error = requestErr.Error()
				step.RawError = requestErr.Error()
				traceSteps = append(traceSteps, step)
				if attempt == 0 {
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s 异常(%s)", ch.Name, channel.FailureLabel(wrapped.FailureInfo())))
				return wrapped
			}
			if response.status < http.StatusOK || response.status >= http.StatusMultipleChoices {
				errBytes, _ := io.ReadAll(io.LimitReader(response.body, 4096))
				_ = response.body.Close()
				rawErr := string(errBytes)
				parsedErr := parseUpstreamErrorMessage(rawErr)
				if parsedErr == "" {
					parsedErr = fmt.Sprintf("HTTP %d", response.status)
				}
				upErr := &upstreamStatusError{status: response.status, retryAfter: parseRetryAfter(response.header.Get("Retry-After")), message: parsedErr}
				lastCandidateErr = upErr
				step.StatusCode = response.status
				step.Error = parsedErr
				step.RawError = rawErr
				traceSteps = append(traceSteps, step)
				// Only resend when the outcome could differ. A deterministic 4xx
				// (bad request, bad key, unknown model) would fail identically.
				if attempt == 0 && channel.FailureIsRetryable(upErr.FailureInfo()) {
					if adapted, ok := adaptUnsupportedReasoningEffort(body, parsedErr); ok {
						retryBody = adapted
					}
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s (%d: %s)", ch.Name, response.status, parsedErr))
				return upErr
			}
			// Do not commit a 2xx response to the client until the upstream has
			// produced its first response bytes. A transport failure before that
			// point is still safe to fail over; after bytes are committed, replaying
			// would duplicate assistant output.
			prefix := make([]byte, 4096)
			n, readErr := io.ReadAtLeast(response.body, prefix, 1)
			if n == 0 {
				_ = response.body.Close()
				streamErr := &streamStartError{err: readErr}
				lastCandidateErr = streamErr
				step.StatusCode = response.status
				step.Error = fmt.Sprintf("传输首字节中断: %v", readErr)
				step.RawError = readErr.Error()
				traceSteps = append(traceSteps, step)
				if attempt == 0 && channel.FailureIsRetryable(channel.FailureFromError(streamErr)) {
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s (传输中断)", ch.Name))
				return streamErr
			}
			winningTtft = int(time.Since(startOverall) / time.Millisecond)
			// The request log keeps the client-perceived wait (including any
			// failed candidates), but the channel metric must describe only this
			// channel: measure from this attempt. A non-streaming response has
			// no early first byte — headers arrive after the whole generation —
			// so it leaves the channel's last real TTFT untouched.
			attemptTtft := int(time.Since(attemptStart) / time.Millisecond)
			if isStreamRequest {
				ch.LastTTFTMs = &attemptTtft
			}
			response.body = &prefixedReadCloser{Reader: io.MultiReader(bytes.NewReader(prefix[:n]), response.body), closer: response.body}
			upstream = *response
			winningChannel = ch
			step.StatusCode = response.status
			step.TtftMs = attemptTtft
			step.Succeeded = true
			traceSteps = append(traceSteps, step)

			if requestLog != nil && requestLog.ID > 0 {
				requestLog.ChannelName = ch.Name
				requestLog.UpstreamModel = upstreamModel
				requestLog.Endpoint = ch.Endpoint
				requestLog.TtftMs = winningTtft
				requestLog.LatencyMs = winningTtft
				if len(failedAttempts) > 0 || attempt > 0 {
					requestLog.IsFailover = true
					chain := append([]string{}, failedAttempts...)
					if attempt > 0 {
						chain = append(chain, fmt.Sprintf("%s 首次失败第1次重试成功", ch.Name))
					}
					requestLog.FailoverFrom = strings.Join(chain, " → ")
				}
				traceBytes, _ := json.Marshal(traceSteps)
				requestLog.TraceJSON = string(traceBytes)
				_ = h.store.UpdateLog(context.Background(), requestLog)
			}
			return nil
		}
		return lastCandidateErr
	})

	totalLatency := int(time.Since(startOverall) / time.Millisecond)

	if routeErr != nil {
		failureInfo := channel.FailureFromError(routeErr)
		responseStatus := http.StatusBadGateway
		if failureInfo.StatusCode == http.StatusTooManyRequests {
			responseStatus = http.StatusTooManyRequests
		}
		if failureInfo.RetryAfter > 0 {
			c.Header("Retry-After", strconv.Itoa(int(failureInfo.RetryAfter/time.Second)))
		}
		failoverDesc := ""
		if len(failedAttempts) > 0 {
			failoverDesc = strings.Join(failedAttempts, " → ")
		}
		traceBytes, _ := json.Marshal(traceSteps)
		if requestLog != nil && requestLog.ID > 0 {
			requestLog.ChannelName = "全渠道失败"
			requestLog.StatusCode = responseStatus
			requestLog.LatencyMs = totalLatency
			requestLog.TtftMs = totalLatency
			requestLog.ErrorMessage = routeErr.Error()
			requestLog.FailoverFrom = failoverDesc
			requestLog.IsFailover = len(failedAttempts) > 1
			requestLog.TraceJSON = string(traceBytes)
			_ = h.store.UpdateLog(context.Background(), requestLog)
		} else if h.store != nil {
			_ = h.store.RecordLog(context.Background(), &channel.RequestLog{
				Model:            effectiveModel,
				ChannelName:      "全渠道失败",
				StatusCode:       responseStatus,
				LatencyMs:        totalLatency,
				TtftMs:           totalLatency,
				PromptTokens:     initialPromptTokens,
				CompletionTokens: 0,
				TotalTokens:      initialPromptTokens,
				IsStream:         isStreamRequest,
				ClientIP:         clientIP,
				UserAgent:        userAgent,
				RequestPath:      requestPath,
				ErrorMessage:     routeErr.Error(),
				FailoverFrom:     failoverDesc,
				IsFailover:       len(failedAttempts) > 1,
				TraceJSON:        string(traceBytes),
				CreatedAt:        time.Now().UTC(),
			})
		}
		if proto == protocolAnthropic {
			c.JSON(responseStatus, gin.H{
				"type": "error",
				"error": gin.H{
					"type":    "api_error",
					"message": fmt.Sprintf("All configured channels failed for model %q (%s): %s", effectiveModel, strings.Join(failedAttempts, " -> "), routeErr.Error()),
				},
			})
			return
		}
		c.JSON(responseStatus, gin.H{"error": "all configured channels failed", "failure_class": channel.FailureLabel(failureInfo)})
		return
	}

	channelName := "未知渠道"
	endpointUrl := ""
	if winChan != nil {
		channelName = winChan.Name
		endpointUrl = winChan.Endpoint
	} else if winningChannel != nil {
		channelName = winningChannel.Name
		endpointUrl = winningChannel.Endpoint
	}

	failoverFrom := ""
	isFailover := false
	if len(failedAttempts) > 0 {
		failoverFrom = strings.Join(failedAttempts, " → ")
		isFailover = true
	}
	isStream := isStreamRequest || strings.Contains(strings.ToLower(upstream.header.Get("Content-Type")), "text/event-stream")

	if requestLog != nil && requestLog.ID > 0 {
		requestLog.ChannelName = channelName
		requestLog.Endpoint = endpointUrl
		requestLog.IsStream = isStream
		requestLog.IsFailover = isFailover
		requestLog.FailoverFrom = failoverFrom
		if requestLog.TtftMs == 0 {
			requestLog.TtftMs = winningTtft
		}
		traceBytes, _ := json.Marshal(traceSteps)
		requestLog.TraceJSON = string(traceBytes)
		_ = h.store.UpdateLog(context.Background(), requestLog)
	}

	for key, values := range upstream.header {
		for _, value := range values {
			c.Writer.Header().Add(key, value)
		}
	}
	c.Header("X-HeiGate-Channel", channelName)
	c.Header("X-HeiGate-Strategy", strategy)
	c.Header("X-HeiGate-Latency-Ms", strconv.Itoa(totalLatency))
	if failoverFrom != "" {
		c.Header("X-HeiGate-Failover", failoverFrom)
	}
	c.Status(upstream.status)
	defer func() { _ = upstream.body.Close() }()
	flusher, isFlusher := c.Writer.(http.Flusher)
	buf := make([]byte, 4096)
	var streamErr error
	tracker := &streamTokenTracker{
		promptTokens: initialPromptTokens,
	}
	lastDbUpdate := time.Now()
	lastReportedTokens := 0
	var nonStreamBuf bytes.Buffer

	for {
		n, readErr := upstream.body.Read(buf)
		if n > 0 {
			if isStream {
				tracker.Feed(buf[:n])
				if requestLog != nil && requestLog.ID > 0 {
					now := time.Now()
					if tracker.completionTokens != lastReportedTokens && now.Sub(lastDbUpdate) >= 200*time.Millisecond {
						requestLog.CompletionTokens = tracker.completionTokens
						if tracker.promptTokens > 0 {
							requestLog.PromptTokens = tracker.promptTokens
						}
						requestLog.TotalTokens = requestLog.PromptTokens + requestLog.CompletionTokens
						requestLog.LatencyMs = int(now.Sub(startOverall) / time.Millisecond)
						_ = h.store.UpdateLog(context.Background(), requestLog)
						lastDbUpdate = now
						lastReportedTokens = tracker.completionTokens
					}
				}
			} else {
				if nonStreamBuf.Len() < 64*1024 {
					_, _ = nonStreamBuf.Write(buf[:n])
				}
			}

			if _, writeErr := c.Writer.Write(buf[:n]); writeErr != nil {
				streamErr = writeErr
				break
			}
			if isFlusher {
				flusher.Flush()
			}
		}
		if readErr != nil {
			if readErr != io.EOF {
				streamErr = readErr
			}
			break
		}
	}

	if isStream && len(tracker.leftover) > 0 {
		tracker.Feed([]byte("\n"))
	}
	if isStream && streamErr == nil && tracker.streamError != "" {
		streamErr = &upstreamStatusError{status: tracker.streamErrorStatus, message: tracker.streamError}
	}
	// A stream that reaches EOF without an upstream completion marker is a
	// truncated response. Do not record it as a successful stop: clients may
	// otherwise accept partial output as a complete answer.
	if isStream && streamErr == nil && !tracker.sawDone && tracker.finishReason == "" {
		streamErr = io.ErrUnexpectedEOF
	}

	isClientAbort := false
	if streamErr != nil {
		if c.Request.Context().Err() != nil || isClientClosedConn(streamErr) {
			isClientAbort = true
		}
	}

	if isStream && streamErr != nil && !isClientAbort && winningChannel != nil && h.store != nil {
		streamError := streamErr.Error()
		_ = h.store.UpdateRuntime(context.Background(), winningChannel.ID, func(fresh *channel.Channel) {
			fresh.FailureCount++
			fresh.LastStatus = probe.StatusError
			fresh.LastError = "流式传输中断: " + streamError
			fresh.LastLatencyMs = nil
			fresh.LastTTFTMs = winningChannel.LastTTFTMs
			now := time.Now().UTC()
			fresh.LastCheckedAt = &now
			failureInfo := channel.FailureFromError(streamErr)
			if failureInfo.Class == "" {
				failureInfo = channel.FailureInfo{Class: channel.FailureNetwork}
			}
			cooldown := channel.FailureCooldown(failureInfo, fresh.FailureCount)
			until := time.Now().UTC().Add(cooldown)
			fresh.CooldownUntil = &until
			if channel.FailureIsCircuitEligible(failureInfo) && fresh.FailureCount >= 3 {
				fresh.CircuitState = channel.CircuitOpen
			} else if fresh.CircuitState == "" {
				fresh.CircuitState = channel.CircuitClosed
			}
		})
	}

	if requestLog != nil {
		requestLog.LatencyMs = int(time.Since(startOverall) / time.Millisecond)
		if isStream {
			if tracker.promptTokens > 0 {
				requestLog.PromptTokens = tracker.promptTokens
			}
			requestLog.CompletionTokens = tracker.completionTokens
			if tracker.totalTokens > 0 {
				requestLog.TotalTokens = tracker.totalTokens
			} else {
				requestLog.TotalTokens = requestLog.PromptTokens + requestLog.CompletionTokens
			}

			if streamErr != nil && !isClientAbort {
				requestLog.StatusCode = http.StatusBadGateway
				requestLog.FinishReason = "error"
				requestLog.ErrorMessage = "流式传输中断: " + streamErr.Error()
			} else if tracker.sawDone || tracker.finishReason != "" {
				requestLog.StatusCode = upstream.status
				requestLog.ErrorMessage = ""
				if tracker.finishReason != "" {
					requestLog.FinishReason = tracker.finishReason
				} else {
					requestLog.FinishReason = "stop"
				}
			} else if isClientAbort && tracker.completionTokens > 0 {
				requestLog.StatusCode = http.StatusOK
				requestLog.FinishReason = "client_abort"
				requestLog.ErrorMessage = fmt.Sprintf("客户端在接收 %d 个 Token 后主动断开连接 (正常完成或停止生成)", tracker.completionTokens)
			} else if streamErr != nil {
				requestLog.StatusCode = http.StatusBadGateway
				requestLog.FinishReason = "error"
				requestLog.ErrorMessage = "流式传输中断: " + streamErr.Error()
			} else {
				requestLog.StatusCode = upstream.status
				requestLog.FinishReason = "stop"
			}
		} else {
			p, c, tot := extractNonStreamingTokens(nonStreamBuf.Bytes())
			if p > 0 {
				requestLog.PromptTokens = p
			}
			if c > 0 {
				requestLog.CompletionTokens = c
			}
			if tot > 0 {
				requestLog.TotalTokens = tot
			} else {
				requestLog.TotalTokens = requestLog.PromptTokens + requestLog.CompletionTokens
			}
			if streamErr != nil {
				requestLog.StatusCode = http.StatusBadGateway
				requestLog.FinishReason = "error"
				requestLog.ErrorMessage = "传输中断: " + streamErr.Error()
			} else {
				requestLog.StatusCode = upstream.status
				requestLog.FinishReason = "stop"
			}
		}

		traceBytes, _ := json.Marshal(traceSteps)
		requestLog.TraceJSON = string(traceBytes)

		if requestLog.ID > 0 {
			_ = h.store.UpdateLog(context.Background(), requestLog)
		} else if h.store != nil {
			_ = h.store.RecordLog(context.Background(), requestLog)
		}
	}
}

type upstreamStatusError struct {
	status     int
	retryAfter time.Duration
	message    string
}

func (e *upstreamStatusError) Error() string {
	if e.message != "" {
		return fmt.Sprintf("upstream returned HTTP %d: %s", e.status, e.message)
	}
	return fmt.Sprintf("upstream returned HTTP %d", e.status)
}

func (e *upstreamStatusError) FailureInfo() channel.FailureInfo {
	info := channel.ClassifyStatus(e.status)
	info.RetryAfter = e.retryAfter
	return info
}

type transportError struct{ err error }

func (e *transportError) Error() string { return "upstream request failed" }
func (e *transportError) Unwrap() error { return e.err }
func (e *transportError) FailureInfo() channel.FailureInfo {
	if info := channel.FailureFromError(e.err); info.Class != "" || info.StatusCode != 0 {
		return info
	}
	return channel.FailureInfo{Class: channel.FailureNetwork}
}

type streamStartError struct{ err error }

func (e *streamStartError) Error() string { return "upstream stream failed before first byte" }
func (e *streamStartError) Unwrap() error { return e.err }
func (e *streamStartError) FailureInfo() channel.FailureInfo {
	return channel.FailureFromError(e.err)
}

type prefixedReadCloser struct {
	io.Reader
	closer io.Closer
}

func (r *prefixedReadCloser) Close() error { return r.closer.Close() }

func parseRetryAfter(value string) time.Duration {
	value = strings.TrimSpace(value)
	if value == "" {
		return 0
	}
	if seconds, err := strconv.Atoi(value); err == nil && seconds > 0 {
		return time.Duration(seconds) * time.Second
	}
	if when, err := http.ParseTime(value); err == nil {
		if delay := time.Until(when); delay > 0 {
			return delay
		}
	}
	return 0
}

func (h *Handler) gatewayModels(c *gin.Context) {
	channels, err := h.store.List(c.Request.Context())
	if err != nil {
		writeError(c, err)
		return
	}
	type model struct {
		ID      string `json:"id"`
		Object  string `json:"object"`
		Created int64  `json:"created,omitempty"`
		OwnedBy string `json:"owned_by"`
	}

	if c.Query("raw") == "true" || c.Query("raw") == "1" {
		seen := make(map[string]struct{})
		data := make([]model, 0)
		hasEnabled := false
		for _, ch := range channels {
			if ch.Enabled {
				hasEnabled = true
				break
			}
		}
		if hasEnabled {
			data = append(data, model{ID: autoModelName, Object: "model", OwnedBy: "heigate"})
		}
		for _, ch := range channels {
			if !ch.Enabled {
				continue
			}
			for _, name := range append([]string{ch.PrimaryModel}, ch.ExtraModels...) {
				name = strings.TrimSpace(name)
				if name == "" {
					continue
				}
				if _, ok := seen[name]; !ok {
					seen[name] = struct{}{}
					data = append(data, model{ID: name, Object: "model", OwnedBy: ch.Name})
				}
			}
		}
		c.JSON(http.StatusOK, gin.H{"object": "list", "data": data})
		return
	}

	// Canonical Aggregated Models
	type groupInfo struct {
		aliases []string
	}
	grouped := make(map[string]*groupInfo)
	order := make([]string, 0)

	for _, ch := range channels {
		if !ch.Enabled {
			continue
		}
		for _, name := range append([]string{ch.PrimaryModel}, ch.ExtraModels...) {
			name = strings.TrimSpace(name)
			if name == "" {
				continue
			}
			groupKey := channel.GetCanonicalGroupKey(name)
			if groupKey == "" {
				continue
			}
			g, exists := grouped[groupKey]
			if !exists {
				g = &groupInfo{aliases: make([]string, 0, 4)}
				grouped[groupKey] = g
				order = append(order, groupKey)
			}
			g.aliases = append(g.aliases, name)
		}
	}

	data := make([]model, 0, len(grouped))
	if len(grouped) > 0 {
		data = append(data, model{ID: autoModelName, Object: "model", Created: time.Now().Unix(), OwnedBy: "heigate"})
	}
	seenCanonical := make(map[string]struct{})
	nowUnix := time.Now().Unix()

	for _, key := range order {
		g := grouped[key]
		canonicalName := channel.SelectCanonicalDisplayName(g.aliases)
		if canonicalName == "" {
			canonicalName = key
		}
		if _, seen := seenCanonical[canonicalName]; seen {
			continue
		}
		seenCanonical[canonicalName] = struct{}{}

		// Determine clean owned_by
		ownedBy := "heigate"
		lower := strings.ToLower(canonicalName)
		switch {
		case strings.HasPrefix(lower, "deepseek"):
			ownedBy = "deepseek"
		case strings.HasPrefix(lower, "claude"):
			ownedBy = "anthropic"
		case strings.HasPrefix(lower, "gpt"):
			ownedBy = "openai"
		case strings.HasPrefix(lower, "qwen"):
			ownedBy = "qwen"
		case strings.HasPrefix(lower, "glm"):
			ownedBy = "zhipu"
		case strings.HasPrefix(lower, "kimi"):
			ownedBy = "moonshot"
		case strings.HasPrefix(lower, "minimax"):
			ownedBy = "minimax"
		case strings.HasPrefix(lower, "grok"):
			ownedBy = "xai"
		}

		data = append(data, model{
			ID:      canonicalName,
			Object:  "model",
			Created: nowUnix,
			OwnedBy: ownedBy,
		})
	}

	// Sort alphabetically for clean display in client tools
	sort.Slice(data, func(i, j int) bool {
		return data[i].ID < data[j].ID
	})

	c.JSON(http.StatusOK, gin.H{"object": "list", "data": data})
}

func requestModel(body []byte) (string, error) {
	var payload struct {
		Model string `json:"model"`
	}
	if err := json.Unmarshal(body, &payload); err != nil {
		return "", fmt.Errorf("invalid JSON request: %w", err)
	}
	if strings.TrimSpace(payload.Model) == "" {
		return "", fmt.Errorf("model is required")
	}
	return strings.TrimSpace(payload.Model), nil
}

func gatewayCandidateMatcher(proto gatewayProtocol) func(*channel.Channel) bool {
	return func(ch *channel.Channel) bool {
		switch proto {
		case protocolAnthropic:
			return ch.Provider == probe.ProviderAnthropic || ch.Provider == probe.ProviderOpenAI
		case protocolChat:
			return ch.Provider == probe.ProviderOpenAI && ch.APIMode == probe.APIModeChatCompletions
		case protocolResponses:
			return ch.Provider == probe.ProviderOpenAI && ch.APIMode == probe.APIModeResponses
		}
		return false
	}
}

func forwardUpstream(ctx context.Context, inbound *http.Request, body []byte, ch *channel.Channel, proto gatewayProtocol, clientModel string, optUpstreamModel ...string) (*upstreamResponse, error) {
	upstreamModel := clientModel
	if len(optUpstreamModel) > 0 && optUpstreamModel[0] != "" {
		upstreamModel = optUpstreamModel[0]
	}
	if clientModel == "" {
		clientModel = upstreamModel
	}

	// Case A: the client speaks Anthropic Messages but the channel is an
	// OpenAI-compatible provider, so the request and response are translated.
	if proto == protocolAnthropic && ch.Provider == probe.ProviderOpenAI {
		return forwardTranslated(ctx, inbound, body, ch, clientModel, upstreamModel)
	}

	// Case B: same protocol on both sides, forward with the model rewritten.
	path := "/v1/chat/completions"
	switch proto {
	case protocolAnthropic:
		path = "/v1/messages"
	case protocolResponses:
		path = "/v1/responses"
	}
	target := probe.NormalizeEndpoint(ch.Endpoint) + path

	sendBody := body
	var payload map[string]any
	if json.Unmarshal(body, &payload) == nil && payload != nil {
		modified := false
		if upstreamModel != "" {
			payload["model"] = upstreamModel
			modified = true
		}
		if streamVal, ok := payload["stream"].(bool); ok && streamVal && proto != protocolAnthropic {
			if _, hasOpt := payload["stream_options"]; !hasOpt {
				payload["stream_options"] = map[string]any{"include_usage": true}
				modified = true
			}
		}
		if modified {
			if newBytes, err := json.Marshal(payload); err == nil {
				sendBody = newBytes
			}
		}
	}

	request, err := http.NewRequestWithContext(ctx, http.MethodPost, target, bytes.NewReader(sendBody))
	if err != nil {
		return nil, err
	}
	copyClientHeaders(request.Header, inbound.Header)
	request.Header.Set("Content-Type", "application/json")
	if proto == protocolAnthropic {
		if ch.APIKey != "" {
			request.Header.Set("x-api-key", ch.APIKey)
		}
		if request.Header.Get("anthropic-version") == "" {
			request.Header.Set("anthropic-version", "2023-06-01")
		}
	} else if ch.APIKey != "" {
		request.Header.Set("Authorization", "Bearer "+ch.APIKey)
	}
	response, err := upstreamClient.Do(request)
	if err != nil {
		return nil, err
	}
	return &upstreamResponse{status: response.StatusCode, header: response.Header.Clone(), body: response.Body}, nil
}

type streamTokenTracker struct {
	promptTokens      int
	completionTokens  int
	totalTokens       int
	hasExactTokens    bool
	estimatedChars    int
	leftover          string
	sawDone           bool
	finishReason      string
	streamError       string
	streamErrorStatus int
}

func (t *streamTokenTracker) Feed(chunk []byte) {
	if len(chunk) == 0 {
		return
	}
	text := t.leftover + string(chunk)
	lines := strings.Split(text, "\n")
	t.leftover = lines[len(lines)-1]

	for i := 0; i < len(lines)-1; i++ {
		line := strings.TrimSpace(lines[i])
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		payload := strings.TrimSpace(strings.TrimPrefix(line, "data:"))
		if payload == "[DONE]" {
			t.sawDone = true
			continue
		}
		if payload == "" {
			continue
		}

		var generic map[string]any
		if err := json.Unmarshal([]byte(payload), &generic); err != nil {
			continue
		}
		if message, status, isError := upstreamErrorPayload([]byte(payload)); isError {
			t.streamError = message
			t.streamErrorStatus = status
			continue
		}

		// Check for finish_reason in choices
		if choices, ok := generic["choices"].([]any); ok && len(choices) > 0 {
			if choice0, ok := choices[0].(map[string]any); ok {
				if fr, ok := choice0["finish_reason"].(string); ok && fr != "" {
					t.finishReason = fr
					if fr == "stop" || fr == "length" || fr == "tool_calls" {
						t.sawDone = true
					}
				}
			}
		}

		// Anthropic message_stop
		if typ, _ := generic["type"].(string); typ == "message_stop" {
			t.sawDone = true
			if t.finishReason == "" {
				t.finishReason = "stop"
			}
		}

		// OpenAI Responses API terminates a stream with a typed
		// response.completed event rather than data: [DONE]. Treat it as the
		// authoritative completion marker so normal Codex traffic is not
		// recorded as a truncated response.
		if typ, _ := generic["type"].(string); typ == "response.completed" {
			t.sawDone = true
			if t.finishReason == "" {
				t.finishReason = "stop"
			}
		}

		// 1. Standard OpenAI usage in chunk
		if usage, ok := generic["usage"].(map[string]any); ok && usage != nil {
			pTokens := getIntField(usage, "prompt_tokens", "input_tokens")
			cTokens := getIntField(usage, "completion_tokens", "output_tokens")
			totTokens := getIntField(usage, "total_tokens")
			if pTokens > 0 {
				t.promptTokens = pTokens
			}
			if cTokens > 0 {
				t.completionTokens = cTokens
			}
			if totTokens > 0 {
				t.totalTokens = totTokens
			} else if t.promptTokens > 0 || t.completionTokens > 0 {
				t.totalTokens = t.promptTokens + t.completionTokens
			}
			t.hasExactTokens = true
			continue
		}

		// 2. Anthropic message_start usage
		if typ, _ := generic["type"].(string); typ == "message_start" {
			if msg, ok := generic["message"].(map[string]any); ok {
				if usage, ok := msg["usage"].(map[string]any); ok {
					pTokens := getIntField(usage, "input_tokens", "prompt_tokens")
					if pTokens > 0 {
						t.promptTokens = pTokens
					}
				}
			}
			continue
		}

		// 3. Anthropic message_delta usage
		if typ, _ := generic["type"].(string); typ == "message_delta" {
			if usage, ok := generic["usage"].(map[string]any); ok {
				cTokens := getIntField(usage, "output_tokens", "completion_tokens")
				if cTokens > 0 {
					t.completionTokens = cTokens
					t.hasExactTokens = true
				}
			}
			continue
		}

		// 4. In-flight token estimation from streaming text chunks (OpenAI or Anthropic)
		if !t.hasExactTokens {
			var deltaContent string
			if choices, ok := generic["choices"].([]any); ok && len(choices) > 0 {
				if choice0, ok := choices[0].(map[string]any); ok {
					if delta, ok := choice0["delta"].(map[string]any); ok {
						if c, ok := delta["content"].(string); ok {
							deltaContent += c
						}
						if r, ok := delta["reasoning_content"].(string); ok {
							deltaContent += r
						}
					}
				}
			}
			if typ, _ := generic["type"].(string); typ == "content_block_delta" {
				if delta, ok := generic["delta"].(map[string]any); ok {
					if txt, ok := delta["text"].(string); ok {
						deltaContent += txt
					}
					if txt, ok := delta["thinking"].(string); ok {
						deltaContent += txt
					}
				}
			}
			if len(deltaContent) > 0 {
				t.estimatedChars += len(deltaContent)
				runes := []rune(deltaContent)
				tokenInc := 0
				for _, r := range runes {
					if r > 0x7F {
						tokenInc++
					}
				}
				asciiCount := len(runes) - tokenInc
				tokenInc += (asciiCount + 3) / 4
				if tokenInc < 1 {
					tokenInc = 1
				}
				t.completionTokens += tokenInc
			}
		}
	}
}

func getIntField(m map[string]any, keys ...string) int {
	for _, k := range keys {
		if v, ok := m[k]; ok {
			switch n := v.(type) {
			case float64:
				return int(n)
			case int:
				return n
			case int64:
				return int(n)
			}
		}
	}
	return 0
}

func estimatePromptTokens(body []byte) int {
	if len(body) == 0 {
		return 0
	}
	var payload struct {
		Prompt   any `json:"prompt"`
		Messages []struct {
			Content any `json:"content"`
		} `json:"messages"`
		System any `json:"system"`
	}
	if err := json.Unmarshal(body, &payload); err != nil {
		chars := len(body)
		if chars/4 < 1 {
			return 1
		}
		return chars / 4
	}
	totalChars := 0
	countAny := func(v any) {
		switch val := v.(type) {
		case string:
			totalChars += len(val)
		case []any:
			for _, item := range val {
				if str, ok := item.(string); ok {
					totalChars += len(str)
				} else if m, ok := item.(map[string]any); ok {
					if txt, ok := m["text"].(string); ok {
						totalChars += len(txt)
					}
				}
			}
		}
	}
	countAny(payload.Prompt)
	countAny(payload.System)
	for _, m := range payload.Messages {
		countAny(m.Content)
	}
	if totalChars == 0 {
		totalChars = len(body)
	}
	est := totalChars / 3
	if est < 1 {
		return 1
	}
	return est
}

func extractNonStreamingTokens(body []byte) (promptTokens int, completionTokens int, totalTokens int) {
	if len(body) == 0 {
		return 0, 0, 0
	}
	var payload struct {
		Usage struct {
			PromptTokens     int `json:"prompt_tokens"`
			CompletionTokens int `json:"completion_tokens"`
			TotalTokens      int `json:"total_tokens"`
			InputTokens      int `json:"input_tokens"`
			OutputTokens     int `json:"output_tokens"`
		} `json:"usage"`
	}
	if json.Unmarshal(body, &payload) == nil {
		p := payload.Usage.PromptTokens
		if p == 0 {
			p = payload.Usage.InputTokens
		}
		c := payload.Usage.CompletionTokens
		if c == 0 {
			c = payload.Usage.OutputTokens
		}
		tot := payload.Usage.TotalTokens
		if tot == 0 && (p > 0 || c > 0) {
			tot = p + c
		}
		return p, c, tot
	}
	return 0, 0, 0
}

// forwardTranslated sends an Anthropic Messages request to an OpenAI Chat
// Completions upstream and converts the reply (JSON or SSE) back into the
// Anthropic wire format.
func forwardTranslated(ctx context.Context, inbound *http.Request, body []byte, ch *channel.Channel, clientModel, upstreamModel string) (*upstreamResponse, error) {
	var msgReq protocol.MessagesRequest
	if err := json.Unmarshal(body, &msgReq); err != nil {
		return nil, fmt.Errorf("parse anthropic request: %w", err)
	}
	if clientModel != "" {
		msgReq.Model = clientModel
	}
	chatReq, err := protocol.MessagesToChat(&msgReq)
	if err != nil {
		return nil, fmt.Errorf("convert anthropic request: %w", err)
	}
	chatReq.Model = upstreamModel
	chatBody, err := json.Marshal(chatReq)
	if err != nil {
		return nil, fmt.Errorf("encode chat completions request: %w", err)
	}

	target := probe.NormalizeEndpoint(ch.Endpoint) + "/v1/chat/completions"
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, target, bytes.NewReader(chatBody))
	if err != nil {
		return nil, err
	}
	copyClientHeaders(request.Header, inbound.Header)
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("Accept", "application/json, text/event-stream")
	request.Header.Del("anthropic-version")
	request.Header.Del("anthropic-beta")
	if ch.APIKey != "" {
		request.Header.Set("Authorization", "Bearer "+ch.APIKey)
	}

	response, err := upstreamClient.Do(request)
	if err != nil {
		return nil, err
	}

	if response.StatusCode >= 400 {
		errBytes, _ := io.ReadAll(io.LimitReader(response.Body, 8192))
		_ = response.Body.Close()
		message := parseUpstreamErrorMessage(string(errBytes))
		if message == "" {
			message = http.StatusText(response.StatusCode)
		}
		payload, _ := json.Marshal(map[string]any{
			"type": "error",
			"error": map[string]any{
				"type":    anthropicErrorType(response.StatusCode),
				"message": fmt.Sprintf("Upstream returned HTTP %d: %s", response.StatusCode, message),
			},
		})
		hdr := make(http.Header)
		hdr.Set("Content-Type", "application/json")
		if retry := response.Header.Get("Retry-After"); retry != "" {
			hdr.Set("Retry-After", retry)
		}
		return &upstreamResponse{status: response.StatusCode, header: hdr, body: io.NopCloser(bytes.NewReader(payload))}, nil
	}

	if !msgReq.Stream {
		respBytes, err := io.ReadAll(response.Body)
		_ = response.Body.Close()
		if err != nil {
			return nil, fmt.Errorf("read chat completions response: %w", err)
		}
		if message, status, isError := upstreamErrorPayload(respBytes); isError {
			return nil, &upstreamStatusError{status: status, message: message}
		}
		var chatResp protocol.ChatResponse
		if err := json.Unmarshal(respBytes, &chatResp); err != nil {
			return nil, fmt.Errorf("decode chat completions response: %w", err)
		}
		out, err := json.Marshal(protocol.ChatToMessages(&chatResp, msgReq.Model))
		if err != nil {
			return nil, fmt.Errorf("encode anthropic response: %w", err)
		}
		hdr := make(http.Header)
		hdr.Set("Content-Type", "application/json")
		return &upstreamResponse{status: http.StatusOK, header: hdr, body: io.NopCloser(bytes.NewReader(out))}, nil
	}

	// Streaming: translate each OpenAI chunk into Anthropic events on the fly.
	pr, pw := io.Pipe()
	go func() {
		defer func() { _ = response.Body.Close() }()
		state := protocol.NewStreamState(msgReq.Model)
		write := func(events []protocol.Event) bool {
			for _, evt := range events {
				frame, err := protocol.EncodeSSE(evt)
				if err != nil {
					continue
				}
				if _, err := io.WriteString(pw, frame); err != nil {
					return false
				}
			}
			return true
		}
		scanner := bufio.NewScanner(response.Body)
		scanner.Buffer(make([]byte, 0, 64<<10), 4<<20)
		sawDone := false
		sawFinish := false
		for scanner.Scan() {
			payload, done, ok := protocol.ParseSSELine(scanner.Text())
			if !ok {
				continue
			}
			if done {
				sawDone = true
				break
			}
			if message, status, isError := upstreamErrorPayload([]byte(payload)); isError {
				_ = pw.CloseWithError(&upstreamStatusError{status: status, message: message})
				return
			}
			var chunk protocol.ChatChunk
			if err := json.Unmarshal([]byte(payload), &chunk); err != nil {
				continue
			}
			for _, choice := range chunk.Choices {
				if choice.FinishReason != nil && strings.TrimSpace(*choice.FinishReason) != "" {
					sawFinish = true
					break
				}
			}
			if !write(state.Feed(&chunk)) {
				_ = pw.CloseWithError(io.ErrClosedPipe)
				return
			}
		}
		if err := scanner.Err(); err != nil {
			_ = pw.CloseWithError(err)
			return
		}
		if !sawDone && !sawFinish {
			_ = pw.CloseWithError(io.ErrUnexpectedEOF)
			return
		}
		write(state.Finish())
		_ = pw.Close()
	}()

	hdr := make(http.Header)
	hdr.Set("Content-Type", "text/event-stream")
	hdr.Set("Cache-Control", "no-cache")
	hdr.Set("Connection", "keep-alive")
	return &upstreamResponse{status: http.StatusOK, header: hdr, body: pr}, nil
}

// anthropicErrorType maps an HTTP status to the closest Anthropic error type.
func anthropicErrorType(status int) string {
	switch {
	case status == http.StatusUnauthorized || status == http.StatusForbidden:
		return "authentication_error"
	case status == http.StatusNotFound:
		return "not_found_error"
	case status == http.StatusTooManyRequests:
		return "rate_limit_error"
	case status == http.StatusRequestEntityTooLarge:
		return "request_too_large"
	case status >= 500:
		return "api_error"
	case status >= 400:
		return "invalid_request_error"
	}
	return "api_error"
}

// copyClientHeaders forwards only protocol headers that are meaningful to an
// upstream provider. Browser/session headers (cookies, origin, referrer and
// extension-specific metadata) must never leak to a configured supplier.
func copyClientHeaders(dst, src http.Header) {
	for key, values := range src {
		switch strings.ToLower(key) {
		case "accept", "user-agent", "anthropic-version", "anthropic-beta", "openai-organization", "openai-project", "openai-beta":
			// allowed protocol metadata
		default:
			continue
		}
		for _, value := range values {
			dst.Add(key, value)
		}
	}
}
