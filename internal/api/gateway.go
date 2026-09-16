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
	effectiveModel := h.resolveModelAlias(c.Request.Context(), model)
	strategy := h.routeStrategy(c)
	candidates, err := h.router.CandidatesMatching(c.Request.Context(), effectiveModel, strategy, gatewayCandidateMatcher(proto))
	if err != nil {
		// Intelligent fallback: if no channel matches this specific model (e.g. out-of-balance or cooldown),
		// fall back to any operational channel in the system so the client request succeeds!
		fallbackCandidates, fbErr := h.router.CandidatesMatching(c.Request.Context(), "", strategy, gatewayCandidateMatcher(proto))
		if fbErr == nil && len(fallbackCandidates) > 0 {
			candidates = fallbackCandidates
			for _, fb := range candidates {
				if fb.PrimaryModel != "" {
					effectiveModel = fb.PrimaryModel
					break
				}
				if len(fb.ExtraModels) > 0 {
					effectiveModel = fb.ExtraModels[0]
					break
				}
			}
			err = nil
		}
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
		if actual, _, ok := channel.FindChannelModelForRequest(ch, effectiveModel); ok && actual != "" {
			upstreamModel = actual
		}
		var lastCandidateErr error
		for attempt := 0; attempt <= 1; attempt++ {
			if attempt > 0 {
				if ctx.Err() != nil {
					return ctx.Err()
				}
				time.Sleep(100 * time.Millisecond)
			}
			attemptStart := time.Now()
			response, requestErr := forwardUpstream(ctx, c.Request, body, ch, proto, effectiveModel, upstreamModel)
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
			n, readErr := response.body.Read(prefix)
			if n == 0 && readErr != nil && readErr != io.EOF {
				_ = response.body.Close()
				streamErr := &streamStartError{err: readErr}
				lastCandidateErr = streamErr
				step.StatusCode = response.status
				step.Error = fmt.Sprintf("传输首字节中断: %v", readErr)
				step.RawError = readErr.Error()
				traceSteps = append(traceSteps, step)
				if attempt == 0 {
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s (传输中断)", ch.Name))
				return streamErr
			}
			winningTtft = int(time.Since(startOverall) / time.Millisecond)
			response.body = &prefixedReadCloser{Reader: io.MultiReader(bytes.NewReader(prefix[:n]), response.body), closer: response.body}
			upstream = *response
			winningChannel = ch
			step.StatusCode = response.status
			step.TtftMs = winningTtft
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

	if routeErr != nil && h.store != nil {
		// If all candidates for this specific model failed (after retry),
		// attempt fallback to another operational channel that supports a different model!
		allChannels, listErr := h.store.List(c.Request.Context())
		if listErr == nil {
			matcher := gatewayCandidateMatcher(proto)
			var fallbackChannels []*channel.Channel
			for _, altChan := range allChannels {
				if !altChan.Enabled || (winChan != nil && altChan.ID == winChan.ID) {
					continue
				}
				if altChan.LastStatus != probe.StatusOperational && altChan.LastStatus != "healthy" {
					continue
				}
				if matcher != nil && !matcher(altChan) {
					continue
				}
				// Skip channels that already failed during the primary model loop
				alreadyTried := false
				for _, cand := range candidates {
					if cand.ID == altChan.ID {
						alreadyTried = true
						break
					}
				}
				if alreadyTried {
					continue
				}
				// Look for an alternate model not equivalent to requested model
				hasAltModel := false
				for _, m := range append([]string{altChan.PrimaryModel}, altChan.ExtraModels...) {
					m = strings.TrimSpace(m)
					if m != "" && !channel.AreModelsEquivalent(m, effectiveModel) {
						hasAltModel = true
						break
					}
				}
				if hasAltModel {
					fallbackChannels = append(fallbackChannels, altChan)
				}
			}

			// Sort fallback candidates by lowest time-to-first-token first,
			// falling back to total latency when the probe measured none.
			sort.SliceStable(fallbackChannels, func(i, j int) bool {
				latI := 999999
				if fallbackChannels[i].LastTTFTMs != nil && *fallbackChannels[i].LastTTFTMs > 0 {
					latI = *fallbackChannels[i].LastTTFTMs
				} else if fallbackChannels[i].LastLatencyMs != nil && *fallbackChannels[i].LastLatencyMs > 0 {
					latI = *fallbackChannels[i].LastLatencyMs
				}
				latJ := 999999
				if fallbackChannels[j].LastTTFTMs != nil && *fallbackChannels[j].LastTTFTMs > 0 {
					latJ = *fallbackChannels[j].LastTTFTMs
				} else if fallbackChannels[j].LastLatencyMs != nil && *fallbackChannels[j].LastLatencyMs > 0 {
					latJ = *fallbackChannels[j].LastLatencyMs
				}
				return latI < latJ
			})

			for _, altChan := range fallbackChannels {
				altModel := altChan.PrimaryModel
				if altModel == "" || channel.AreModelsEquivalent(altModel, effectiveModel) {
					for _, m := range altChan.ExtraModels {
						m = strings.TrimSpace(m)
						if m != "" && !channel.AreModelsEquivalent(m, effectiveModel) {
							altModel = m
							break
						}
					}
				}
				if altModel == "" {
					continue
				}

				var fallbackSucceeded bool
				for fbAttempt := 0; fbAttempt <= 1; fbAttempt++ {
					if fbAttempt > 0 {
						if c.Request.Context().Err() != nil {
							break
						}
						time.Sleep(100 * time.Millisecond)
					}
					fbStart := time.Now()
					altResp, altErr := forwardUpstream(c.Request.Context(), c.Request, body, altChan, proto, effectiveModel, altModel)
					fbLatency := int(time.Since(fbStart) / time.Millisecond)
					step := TraceStep{
						Index:       len(traceSteps) + 1,
						ChannelName: altChan.Name,
						ChannelID:   altChan.ID,
						Endpoint:    altChan.Endpoint,
						Model:       altModel,
						Attempt:     fbAttempt + 1,
						LatencyMs:   fbLatency,
					}
					if altErr != nil {
						step.Error = altErr.Error()
						step.RawError = altErr.Error()
						traceSteps = append(traceSteps, step)
						continue
					}
					if altResp.status < http.StatusOK || altResp.status >= http.StatusMultipleChoices {
						errBytes, _ := io.ReadAll(io.LimitReader(altResp.body, 4096))
						_ = altResp.body.Close()
						rawErr := string(errBytes)
						parsedErr := parseUpstreamErrorMessage(rawErr)
						if parsedErr == "" {
							parsedErr = fmt.Sprintf("HTTP %d", altResp.status)
						}
						step.StatusCode = altResp.status
						step.Error = parsedErr
						step.RawError = rawErr
						traceSteps = append(traceSteps, step)
						continue
					}
					prefix := make([]byte, 4096)
					n, rErr := altResp.body.Read(prefix)
					if n > 0 || rErr == nil || rErr == io.EOF {
						winningTtft = int(time.Since(startOverall) / time.Millisecond)
						altResp.body = &prefixedReadCloser{Reader: io.MultiReader(bytes.NewReader(prefix[:n]), altResp.body), closer: altResp.body}
						upstream = *altResp
						winningChannel = altChan
						winChan = altChan
						failedAttempts = append(failedAttempts, fmt.Sprintf("%s 降级至备用模型 [%s](%s)", effectiveModel, altChan.Name, altModel))
						routeErr = nil
						step.StatusCode = altResp.status
						step.TtftMs = winningTtft
						step.Succeeded = true
						traceSteps = append(traceSteps, step)

						if requestLog != nil && requestLog.ID > 0 {
							requestLog.ChannelName = altChan.Name
							requestLog.UpstreamModel = altModel
							requestLog.Endpoint = altChan.Endpoint
							requestLog.TtftMs = winningTtft
							requestLog.LatencyMs = winningTtft
							requestLog.IsFailover = true
							requestLog.FailoverFrom = strings.Join(failedAttempts, " → ")
							traceBytes, _ := json.Marshal(traceSteps)
							requestLog.TraceJSON = string(traceBytes)
							_ = h.store.UpdateLog(context.Background(), requestLog)
						}
						fallbackSucceeded = true
						break
					}
					// Transport died before the first byte: the next attempt may
					// still succeed, but the abandoned body must be closed or the
					// keep-alive connection leaks (the primary loop closes this
					// path; the fallback loop must too).
					_ = altResp.body.Close()
					step.StatusCode = altResp.status
					step.Error = fmt.Sprintf("传输首字节中断: %v", rErr)
					step.RawError = rErr.Error()
					traceSteps = append(traceSteps, step)
				}
				if fallbackSucceeded {
					break
				}
			}
		}
	}

	totalLatency := int(time.Since(startOverall) / time.Millisecond)

	if routeErr != nil {
		if info := channel.FailureFromError(routeErr); info.RetryAfter > 0 {
			c.Header("Retry-After", strconv.Itoa(int(info.RetryAfter/time.Second)))
		}
		failoverDesc := ""
		if len(failedAttempts) > 0 {
			failoverDesc = strings.Join(failedAttempts, " → ")
		}
		traceBytes, _ := json.Marshal(traceSteps)
		if requestLog != nil && requestLog.ID > 0 {
			requestLog.ChannelName = "全渠道失败"
			requestLog.StatusCode = http.StatusBadGateway
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
				StatusCode:       http.StatusBadGateway,
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
			c.JSON(http.StatusBadGateway, gin.H{
				"type": "error",
				"error": gin.H{
					"type":    "api_error",
					"message": fmt.Sprintf("All configured channels failed for model %q (%s): %s", effectiveModel, strings.Join(failedAttempts, " -> "), routeErr.Error()),
				},
			})
			return
		}
		c.JSON(http.StatusBadGateway, gin.H{"error": "all configured channels failed", "failure_class": channel.FailureLabel(channel.FailureFromError(routeErr))})
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

	isClientAbort := false
	if streamErr != nil {
		if c.Request.Context().Err() != nil || isClientClosedConn(streamErr) {
			isClientAbort = true
		}
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

			if tracker.sawDone || tracker.finishReason != "" {
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
	return channel.FailureInfo{Class: channel.FailureNetwork}
}

type streamStartError struct{ err error }

func (e *streamStartError) Error() string { return "upstream stream failed before first byte" }
func (e *streamStartError) Unwrap() error { return e.err }
func (e *streamStartError) FailureInfo() channel.FailureInfo {
	return channel.FailureInfo{Class: channel.FailureNetwork}
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
	promptTokens     int
	completionTokens int
	totalTokens      int
	hasExactTokens   bool
	estimatedChars   int
	leftover         string
	sawDone          bool
	finishReason     string
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
		for scanner.Scan() {
			payload, done, ok := protocol.ParseSSELine(scanner.Text())
			if !ok {
				continue
			}
			if done {
				break
			}
			var chunk protocol.ChatChunk
			if err := json.Unmarshal([]byte(payload), &chunk); err != nil {
				continue
			}
			if !write(state.Feed(&chunk)) {
				_ = pw.CloseWithError(io.ErrClosedPipe)
				return
			}
		}
		if err := scanner.Err(); err != nil && !isClientClosedConn(err) {
			// The upstream stream broke: still finish the Anthropic message so
			// the client gets a well-formed end, then surface the error.
			write(state.Finish())
			_ = pw.CloseWithError(err)
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

// copyClientHeaders forwards the client's headers to the upstream except the
// ones that carry credentials or are connection specific.
func copyClientHeaders(dst, src http.Header) {
	for key, values := range src {
		switch strings.ToLower(key) {
		case "authorization", "x-api-key", "host", "content-length", "connection", "accept-encoding", "x-heigate-request":
			continue
		}
		for _, value := range values {
			dst.Add(key, value)
		}
	}
}
