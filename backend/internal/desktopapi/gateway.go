package desktopapi

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Wei-Shaw/sub2api/internal/pkg/apicompat"
	"github.com/Wei-Shaw/sub2api/internal/service"
	"github.com/gin-gonic/gin"
)

const desktopGatewayMaxBody = 16 << 20

type desktopGatewayResponse struct {
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
	h.forward(c, desktopGatewayProtocolAnthropic)
}

func (h *Handler) forwardChatCompletions(c *gin.Context) {
	h.forward(c, desktopGatewayProtocolChat)
}

func (h *Handler) forwardResponses(c *gin.Context) {
	h.forward(c, desktopGatewayProtocolResponses)
}

type desktopGatewayProtocol string

const (
	desktopGatewayProtocolAnthropic desktopGatewayProtocol = "anthropic"
	desktopGatewayProtocolChat      desktopGatewayProtocol = "chat_completions"
	desktopGatewayProtocolResponses desktopGatewayProtocol = "responses"
)

func channelSupportsModel(ch *service.DesktopChannel, model string) bool {
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
			// 3. Fallback: if requestedModel starts with "claude-" but no channel has it,
			// pick the first enabled healthy channel's primary model
			if strings.HasPrefix(strings.ToLower(requestedModel), "claude-") || strings.HasPrefix(strings.ToLower(requestedModel), "anthropic/claude-") {
				for _, ch := range channels {
					if ch.Enabled && strings.TrimSpace(ch.PrimaryModel) != "" {
						return strings.TrimSpace(ch.PrimaryModel)
					}
				}
			}
		}
	}
	return requestedModel
}

func (h *Handler) forward(c *gin.Context, protocol desktopGatewayProtocol) {
	body, err := io.ReadAll(io.LimitReader(c.Request.Body, desktopGatewayMaxBody+1))
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "read request body: " + err.Error()})
		return
	}
	if len(body) > desktopGatewayMaxBody {
		c.JSON(http.StatusRequestEntityTooLarge, gin.H{"error": "request body is too large"})
		return
	}
	model, err := desktopRequestModel(body)
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	effectiveModel := h.resolveModelAlias(c.Request.Context(), model)
	strategy := h.routeStrategy(c)
	candidates, err := h.router.CandidatesMatching(c.Request.Context(), effectiveModel, strategy, gatewayCandidateMatcher(protocol))
	if err != nil {
		// Intelligent fallback: if no channel matches this specific model (e.g. out-of-balance or cooldown),
		// fall back to any operational channel in the system so the client request succeeds!
		fallbackCandidates, fbErr := h.router.CandidatesMatching(c.Request.Context(), "", strategy, gatewayCandidateMatcher(protocol))
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
		if protocol == desktopGatewayProtocolAnthropic {
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
	var upstream desktopGatewayResponse
	var winningChannel *service.DesktopChannel
	var failedAttempts []string
	startOverall := time.Now()
	var winningTtft int
	initialPromptTokens := estimatePromptTokens(body)
	isStreamRequest := bytes.Contains(body, []byte(`"stream":true`)) || bytes.Contains(body, []byte(`"stream": true`)) || strings.Contains(strings.ToLower(c.Request.Header.Get("Accept")), "text/event-stream")

	var requestLog *service.DesktopRequestLog
	if h.store != nil {
		requestLog = &service.DesktopRequestLog{
			Model:            effectiveModel,
			ChannelName:      "路由中...",
			StatusCode:       0, // 0 = 传输中
			LatencyMs:        0,
			TtftMs:           0,
			PromptTokens:     initialPromptTokens,
			CompletionTokens: 0,
			TotalTokens:      initialPromptTokens,
			IsStream:         isStreamRequest,
			CreatedAt:        time.Now().UTC(),
		}
		_ = h.store.RecordLog(context.Background(), requestLog)
	}

	winChan, routeErr := h.router.RouteCandidates(c.Request.Context(), effectiveModel, candidates, func(ctx context.Context, channel *service.DesktopChannel) error {
		upstreamModel := effectiveModel
		if actual, _, ok := service.FindChannelModelForRequest(channel, effectiveModel); ok && actual != "" {
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
			response, requestErr := forwardDesktopRequest(ctx, c.Request, body, channel, protocol, effectiveModel, upstreamModel)
			if requestErr != nil {
				wrapped := &desktopRequestError{err: requestErr}
				lastCandidateErr = wrapped
				if attempt == 0 {
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s 异常(%s, 重试1次未果)", channel.Name, service.DesktopFailureLabel(wrapped.FailureInfo())))
				return wrapped
			}
			if response.status < http.StatusOK || response.status >= http.StatusMultipleChoices {
				_, _ = io.Copy(io.Discard, io.LimitReader(response.body, 4096))
				_ = response.body.Close()
				upErr := &desktopUpstreamError{status: response.status, retryAfter: parseRetryAfter(response.header.Get("Retry-After"))}
				lastCandidateErr = upErr
				if attempt == 0 {
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s 响应(%d, 重试1次未果)", channel.Name, response.status))
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
				streamErr := &desktopStreamStartError{err: readErr}
				lastCandidateErr = streamErr
				if attempt == 0 {
					continue
				}
				failedAttempts = append(failedAttempts, fmt.Sprintf("%s 传输中断(重试1次未果)", channel.Name))
				return streamErr
			}
			winningTtft = int(time.Since(startOverall) / time.Millisecond)
			response.body = &prefixedReadCloser{Reader: io.MultiReader(bytes.NewReader(prefix[:n]), response.body), closer: response.body}
			upstream = *response
			winningChannel = channel
			if requestLog != nil && requestLog.ID > 0 {
				requestLog.ChannelName = channel.Name
				requestLog.TtftMs = winningTtft
				requestLog.LatencyMs = winningTtft
				if len(failedAttempts) > 0 || attempt > 0 {
					requestLog.IsFailover = true
					chain := append([]string{}, failedAttempts...)
					if attempt > 0 {
						chain = append(chain, fmt.Sprintf("%s 首次失败第1次重试成功", channel.Name))
					}
					requestLog.FailoverFrom = strings.Join(chain, " → ")
				}
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
			matcher := gatewayCandidateMatcher(protocol)
			var fallbackChannels []*service.DesktopChannel
			for _, altChan := range allChannels {
				if !altChan.Enabled || (winChan != nil && altChan.ID == winChan.ID) {
					continue
				}
				if altChan.LastStatus != service.MonitorStatusOperational && altChan.LastStatus != "healthy" {
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
					if m != "" && !service.AreModelsEquivalent(m, effectiveModel) {
						hasAltModel = true
						break
					}
				}
				if hasAltModel {
					fallbackChannels = append(fallbackChannels, altChan)
				}
			}

			// Sort fallback candidates by lowest latency first
			sort.SliceStable(fallbackChannels, func(i, j int) bool {
				latI := 999999
				if fallbackChannels[i].LastLatencyMs != nil && *fallbackChannels[i].LastLatencyMs > 0 {
					latI = *fallbackChannels[i].LastLatencyMs
				}
				latJ := 999999
				if fallbackChannels[j].LastLatencyMs != nil && *fallbackChannels[j].LastLatencyMs > 0 {
					latJ = *fallbackChannels[j].LastLatencyMs
				}
				return latI < latJ
			})

			for _, altChan := range fallbackChannels {
				altModel := altChan.PrimaryModel
				if altModel == "" || service.AreModelsEquivalent(altModel, effectiveModel) {
					for _, m := range altChan.ExtraModels {
						m = strings.TrimSpace(m)
						if m != "" && !service.AreModelsEquivalent(m, effectiveModel) {
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
					altResp, altErr := forwardDesktopRequest(c.Request.Context(), c.Request, body, altChan, protocol, effectiveModel, altModel)
					if altErr == nil && altResp.status >= http.StatusOK && altResp.status < http.StatusMultipleChoices {
						prefix := make([]byte, 4096)
						n, rErr := altResp.body.Read(prefix)
						if n > 0 || rErr == nil || rErr == io.EOF {
							winningTtft = int(time.Since(startOverall) / time.Millisecond)
							altResp.body = &prefixedReadCloser{Reader: io.MultiReader(bytes.NewReader(prefix[:n]), altResp.body), closer: altResp.body}
							upstream = *altResp
							winningChannel = altChan
							winChan = altChan
							failedAttempts = append(failedAttempts, fmt.Sprintf("%s 同模型供应商全部失败 -> 自动降级至备用模型 [%s](%s)", effectiveModel, altChan.Name, altModel))
							routeErr = nil
							if requestLog != nil && requestLog.ID > 0 {
								requestLog.ChannelName = altChan.Name
								requestLog.TtftMs = winningTtft
								requestLog.LatencyMs = winningTtft
								requestLog.IsFailover = true
								requestLog.FailoverFrom = strings.Join(failedAttempts, " → ")
								_ = h.store.UpdateLog(context.Background(), requestLog)
							}
							fallbackSucceeded = true
							break
						}
					}
				}
				if fallbackSucceeded {
					break
				}
			}
		}
	}

	totalLatency := int(time.Since(startOverall) / time.Millisecond)

	if routeErr != nil {
		if info := service.DesktopFailureFromError(routeErr); info.RetryAfter > 0 {
			c.Header("Retry-After", strconv.Itoa(int(info.RetryAfter/time.Second)))
		}
		failoverDesc := ""
		if len(failedAttempts) > 0 {
			failoverDesc = strings.Join(failedAttempts, " → ")
		}
		if requestLog != nil && requestLog.ID > 0 {
			requestLog.ChannelName = "全渠道失败"
			requestLog.StatusCode = http.StatusBadGateway
			requestLog.LatencyMs = totalLatency
			requestLog.TtftMs = totalLatency
			requestLog.ErrorMessage = routeErr.Error()
			requestLog.FailoverFrom = failoverDesc
			requestLog.IsFailover = len(failedAttempts) > 1
			_ = h.store.UpdateLog(context.Background(), requestLog)
		} else if h.store != nil {
			_ = h.store.RecordLog(context.Background(), &service.DesktopRequestLog{
				Model:            effectiveModel,
				ChannelName:      "全渠道失败",
				StatusCode:       http.StatusBadGateway,
				LatencyMs:        totalLatency,
				TtftMs:           totalLatency,
				PromptTokens:     initialPromptTokens,
				CompletionTokens: 0,
				TotalTokens:      initialPromptTokens,
				IsStream:         isStreamRequest,
				ErrorMessage:     routeErr.Error(),
				FailoverFrom:     failoverDesc,
				IsFailover:       len(failedAttempts) > 1,
				CreatedAt:        time.Now().UTC(),
			})
		}
		if protocol == desktopGatewayProtocolAnthropic {
			c.JSON(http.StatusBadGateway, gin.H{
				"type": "error",
				"error": gin.H{
					"type":    "api_error",
					"message": fmt.Sprintf("All configured channels failed for model %q (%s): %s", effectiveModel, strings.Join(failedAttempts, " -> "), routeErr.Error()),
				},
			})
			return
		}
		c.JSON(http.StatusBadGateway, gin.H{"error": "all configured channels failed", "failure_class": service.DesktopFailureLabel(service.DesktopFailureFromError(routeErr))})
		return
	}

	channelName := "未知渠道"
	if winChan != nil {
		channelName = winChan.Name
	} else if winningChannel != nil {
		channelName = winningChannel.Name
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
		requestLog.IsStream = isStream
		requestLog.IsFailover = isFailover
		requestLog.FailoverFrom = failoverFrom
		if requestLog.TtftMs == 0 {
			requestLog.TtftMs = winningTtft
		}
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
	defer upstream.body.Close()
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
					nonStreamBuf.Write(buf[:n])
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
	if requestLog != nil {
		requestLog.StatusCode = upstream.status
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
		}
		if streamErr != nil {
			requestLog.StatusCode = http.StatusBadGateway
			requestLog.ErrorMessage = "stream interrupted: " + streamErr.Error()
		}
		if requestLog.ID > 0 {
			_ = h.store.UpdateLog(context.Background(), requestLog)
		} else if h.store != nil {
			_ = h.store.RecordLog(context.Background(), requestLog)
		}
	}
}

type desktopUpstreamError struct {
	status     int
	retryAfter time.Duration
}

func (e *desktopUpstreamError) Error() string {
	return fmt.Sprintf("upstream returned HTTP %d", e.status)
}

func (e *desktopUpstreamError) FailureInfo() service.DesktopFailureInfo {
	info := service.DesktopClassifyStatus(e.status)
	info.RetryAfter = e.retryAfter
	return info
}

type desktopRequestError struct{ err error }

func (e *desktopRequestError) Error() string { return "upstream request failed" }
func (e *desktopRequestError) Unwrap() error { return e.err }
func (e *desktopRequestError) FailureInfo() service.DesktopFailureInfo {
	return service.DesktopFailureInfo{Class: service.DesktopFailureNetwork}
}

type desktopStreamStartError struct{ err error }

func (e *desktopStreamStartError) Error() string { return "upstream stream failed before first byte" }
func (e *desktopStreamStartError) Unwrap() error { return e.err }
func (e *desktopStreamStartError) FailureInfo() service.DesktopFailureInfo {
	return service.DesktopFailureInfo{Class: service.DesktopFailureNetwork}
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
		for _, channel := range channels {
			if !channel.Enabled {
				continue
			}
			for _, name := range append([]string{channel.PrimaryModel}, channel.ExtraModels...) {
				name = strings.TrimSpace(name)
				if name == "" {
					continue
				}
				if _, ok := seen[name]; !ok {
					seen[name] = struct{}{}
					data = append(data, model{ID: name, Object: "model", OwnedBy: channel.Name})
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

	for _, channel := range channels {
		if !channel.Enabled {
			continue
		}
		for _, name := range append([]string{channel.PrimaryModel}, channel.ExtraModels...) {
			name = strings.TrimSpace(name)
			if name == "" {
				continue
			}
			groupKey := service.GetCanonicalGroupKey(name)
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
		canonicalName := service.SelectCanonicalDisplayName(g.aliases)
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

func desktopRequestModel(body []byte) (string, error) {
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

func gatewayCandidateMatcher(protocol desktopGatewayProtocol) func(*service.DesktopChannel) bool {
	return func(channel *service.DesktopChannel) bool {
		switch protocol {
		case desktopGatewayProtocolAnthropic:
			return channel.Provider == service.MonitorProviderAnthropic || channel.Provider == service.MonitorProviderOpenAI
		case desktopGatewayProtocolChat:
			return channel.Provider == service.MonitorProviderOpenAI && channel.APIMode == service.MonitorAPIModeChatCompletions
		case desktopGatewayProtocolResponses:
			return channel.Provider == service.MonitorProviderOpenAI && channel.APIMode == service.MonitorAPIModeResponses
		}
		return false
	}
}

func forwardDesktopRequest(ctx context.Context, inbound *http.Request, body []byte, channel *service.DesktopChannel, protocol desktopGatewayProtocol, clientModel string, optUpstreamModel ...string) (*desktopGatewayResponse, error) {
	upstreamModel := clientModel
	if len(optUpstreamModel) > 0 && optUpstreamModel[0] != "" {
		upstreamModel = optUpstreamModel[0]
	}
	if clientModel == "" {
		clientModel = upstreamModel
	}

	// Case A: Anthropic protocol targeting OpenAI-compatible provider -> Protocol conversion
	if protocol == desktopGatewayProtocolAnthropic && channel.Provider == service.MonitorProviderOpenAI {
		var anthropicReq apicompat.AnthropicRequest
		if err := json.Unmarshal(body, &anthropicReq); err != nil {
			return nil, fmt.Errorf("parse anthropic request: %w", err)
		}
		if clientModel != "" {
			anthropicReq.Model = clientModel
		}
		chatReq, err := apicompat.AnthropicToChatCompletionsRequest(&anthropicReq)
		if err != nil {
			return nil, fmt.Errorf("convert anthropic to chat completions: %w", err)
		}
		chatReq.Model = upstreamModel
		chatReq.Stream = anthropicReq.Stream
		if anthropicReq.Stream {
			chatReq.StreamOptions = &apicompat.ChatStreamOptions{IncludeUsage: true}
		}
		chatBody, err := json.Marshal(chatReq)
		if err != nil {
			return nil, fmt.Errorf("marshal chat completions request: %w", err)
		}

		baseURL := strings.TrimRight(strings.TrimSpace(channel.Endpoint), "/")
		baseURL = strings.TrimSuffix(baseURL, "/v1")
		target := baseURL + "/v1/chat/completions"

		request, err := http.NewRequestWithContext(ctx, http.MethodPost, target, bytes.NewReader(chatBody))
		if err != nil {
			return nil, err
		}
		for key, values := range inbound.Header {
			if strings.EqualFold(key, "Authorization") || strings.EqualFold(key, "X-Api-Key") || strings.EqualFold(key, "Host") || strings.EqualFold(key, "Content-Length") {
				continue
			}
			for _, value := range values {
				request.Header.Add(key, value)
			}
		}
		request.Header.Set("Content-Type", "application/json")
		if channel.APIKey != "" {
			request.Header.Set("Authorization", "Bearer "+channel.APIKey)
		}

		client := &http.Client{Timeout: 10 * time.Minute}
		response, err := client.Do(request)
		if err != nil {
			return nil, err
		}

		if response.StatusCode >= 400 {
			errBytes, _ := io.ReadAll(io.LimitReader(response.Body, 8192))
			_ = response.Body.Close()
			var errMsg string
			var jsonErr struct {
				Error struct {
					Message string `json:"message"`
				} `json:"error"`
			}
			if json.Unmarshal(errBytes, &jsonErr) == nil && jsonErr.Error.Message != "" {
				errMsg = jsonErr.Error.Message
			} else {
				errMsg = string(errBytes)
			}
			anthropicErr := map[string]any{
				"type": "error",
				"error": map[string]any{
					"type":    "api_error",
					"message": fmt.Sprintf("Upstream returned HTTP %d: %s", response.StatusCode, errMsg),
				},
			}
			errPayload, _ := json.Marshal(anthropicErr)
			hdr := make(http.Header)
			hdr.Set("Content-Type", "application/json")
			return &desktopGatewayResponse{
				status: response.StatusCode,
				header: hdr,
				body:   io.NopCloser(bytes.NewReader(errPayload)),
			}, nil
		}

		if !anthropicReq.Stream {
			respBytes, err := io.ReadAll(response.Body)
			_ = response.Body.Close()
			if err != nil {
				return nil, fmt.Errorf("read chat response: %w", err)
			}
			var ccResp apicompat.ChatCompletionsResponse
			if err := json.Unmarshal(respBytes, &ccResp); err != nil {
				return nil, fmt.Errorf("unmarshal chat response: %w", err)
			}
			anthropicResp := apicompat.ChatCompletionsResponseToAnthropic(&ccResp, anthropicReq.Model)
			outBytes, err := json.Marshal(anthropicResp)
			if err != nil {
				return nil, fmt.Errorf("marshal anthropic response: %w", err)
			}
			hdr := make(http.Header)
			hdr.Set("Content-Type", "application/json")
			return &desktopGatewayResponse{
				status: http.StatusOK,
				header: hdr,
				body:   io.NopCloser(bytes.NewReader(outBytes)),
			}, nil
		}

		// Streaming SSE response
		pr, pw := io.Pipe()
		go func() {
			defer response.Body.Close()
			defer pw.Close()

			scanner := bufio.NewScanner(response.Body)
			state := apicompat.NewChatCompletionsToAnthropicStreamState(anthropicReq.Model)
			for scanner.Scan() {
				line := strings.TrimSpace(scanner.Text())
				if !strings.HasPrefix(line, "data:") {
					continue
				}
				payload := strings.TrimSpace(strings.TrimPrefix(line, "data:"))
				if payload == "" {
					continue
				}
				if payload == "[DONE]" {
					break
				}
				var chunk apicompat.ChatCompletionsChunk
				if err := json.Unmarshal([]byte(payload), &chunk); err != nil {
					continue
				}
				events := apicompat.ChatCompletionsChunkToAnthropicEvents(&chunk, state)
				for _, evt := range events {
					sse, err := apicompat.ResponsesAnthropicEventToSSE(evt)
					if err == nil {
						if _, err := io.WriteString(pw, sse); err != nil {
							return
						}
					}
				}
			}
			finalEvents := apicompat.FinalizeChatCompletionsAnthropicStream(state)
			for _, evt := range finalEvents {
				sse, err := apicompat.ResponsesAnthropicEventToSSE(evt)
				if err == nil {
					if _, err := io.WriteString(pw, sse); err != nil {
						return
					}
				}
			}
		}()

		hdr := make(http.Header)
		hdr.Set("Content-Type", "text/event-stream")
		hdr.Set("Cache-Control", "no-cache")
		hdr.Set("Connection", "keep-alive")
		return &desktopGatewayResponse{
			status: http.StatusOK,
			header: hdr,
			body:   pr,
		}, nil
	}

	// Case B: Direct forwarding
	path := "/v1/chat/completions"
	if protocol == desktopGatewayProtocolAnthropic {
		path = "/v1/messages"
	} else if protocol == desktopGatewayProtocolResponses {
		path = "/v1/responses"
	}
	baseURL := strings.TrimRight(strings.TrimSpace(channel.Endpoint), "/")
	baseURL = strings.TrimSuffix(baseURL, "/v1")
	target := baseURL + path

	sendBody := body
	var payload map[string]any
	if json.Unmarshal(body, &payload) == nil && payload != nil {
		modified := false
		if upstreamModel != "" {
			payload["model"] = upstreamModel
			modified = true
		}
		if streamVal, ok := payload["stream"].(bool); ok && streamVal && protocol != desktopGatewayProtocolAnthropic {
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
	for key, values := range inbound.Header {
		if strings.EqualFold(key, "Authorization") || strings.EqualFold(key, "X-Api-Key") || strings.EqualFold(key, "Host") || strings.EqualFold(key, "Content-Length") {
			continue
		}
		for _, value := range values {
			request.Header.Add(key, value)
		}
	}
	request.Header.Set("Content-Type", "application/json")
	if protocol == desktopGatewayProtocolAnthropic {
		if channel.APIKey != "" {
			request.Header.Set("x-api-key", channel.APIKey)
		}
		if request.Header.Get("anthropic-version") == "" {
			request.Header.Set("anthropic-version", "2023-06-01")
		}
	} else {
		if channel.APIKey != "" {
			request.Header.Set("Authorization", "Bearer "+channel.APIKey)
		}
	}
	client := &http.Client{Timeout: 10 * time.Minute}
	response, err := client.Do(request)
	if err != nil {
		return nil, err
	}
	return &desktopGatewayResponse{status: response.StatusCode, header: response.Header.Clone(), body: response.Body}, nil
}

type streamTokenTracker struct {
	promptTokens     int
	completionTokens int
	totalTokens      int
	hasExactTokens   bool
	estimatedChars   int
	leftover         string
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
		if payload == "" || payload == "[DONE]" {
			continue
		}

		var generic map[string]any
		if err := json.Unmarshal([]byte(payload), &generic); err != nil {
			continue
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
