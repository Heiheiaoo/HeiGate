package api

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Heiheiaoo/heigate/internal/channel"
	"github.com/Heiheiaoo/heigate/internal/datadir"
	"github.com/Heiheiaoo/heigate/internal/probe"
	"github.com/gin-gonic/gin"
)

// Handler exposes the local-only channel management API used by the desktop UI.
// Authentication is deliberately not part of this handler; callers must mount
// it behind LoopbackOnly so it cannot become a public unauthenticated API.
type Handler struct {
	store   *channel.Store
	runner  *channel.ProbeRunner
	router  *channel.Router
	key     string
	baseURL string // e.g. http://127.0.0.1:8080, advertised to client integrations
}

func NewHandler(store *channel.Store, runner *channel.ProbeRunner) *Handler {
	return NewHandlerWithGatewayKey(store, runner, "")
}

func NewHandlerWithGatewayKey(store *channel.Store, runner *channel.ProbeRunner, key string) *Handler {
	h := &Handler{store: store, runner: runner, router: channel.NewRouter(store), key: key, baseURL: "http://127.0.0.1:8080"}
	if store != nil {
		if raw, err := store.GetSetting(context.Background(), "custom_model_mappings", "{}"); err == nil && raw != "" {
			var mappings map[string]string
			if err := json.Unmarshal([]byte(raw), &mappings); err == nil {
				channel.SetCustomModelMappings(mappings)
			}
		}
	}
	return h
}

// SetGatewayBaseURL records the address clients should use to reach the
// gateway; it is written into Claude / Codex configuration files.
func (h *Handler) SetGatewayBaseURL(baseURL string) {
	if u := strings.TrimRight(strings.TrimSpace(baseURL), "/"); u != "" {
		h.baseURL = u
	}
}

// RegisterRoutes mounts the local management API below the supplied group.
// The caller must protect the group with middleware.SameOriginOnly.
func (h *Handler) RegisterRoutes(group *gin.RouterGroup) {
	group.GET("/session", h.session)
	group.GET("/channels", h.list)
	group.GET("/channels/candidates", h.candidates)
	group.POST("/channels", h.create)
	group.GET("/channels/:id", h.get)
	group.PUT("/channels/:id", h.update)
	group.DELETE("/channels/:id", h.remove)
	group.POST("/channels/:id/probe", h.probe)
	group.POST("/channels/:id/injection-check", h.injectionCheck)
	group.POST("/channels/:id/duplicate", h.duplicate)
	group.POST("/channels/upstream-models", h.fetchUpstreamModels)
	group.GET("/models", h.models)
	group.GET("/custom-model-mappings", h.getCustomModelMappings)
	group.POST("/custom-model-mappings", h.setCustomModelMapping)
	group.DELETE("/custom-model-mappings", h.deleteCustomModelMapping)
	group.GET("/analytics", h.analytics)
	group.GET("/logs", h.getLogs)
	group.GET("/logs/stream", h.streamLogs)
	group.DELETE("/logs", h.clearLogs)
	group.POST("/logs/prune", h.pruneLogs)
	group.GET("/system", h.systemInfo)
	group.PUT("/settings", h.updateSettings)
	group.GET("/clients", h.listClients)
	group.POST("/clients/claude/configure", h.configureClaude)
	group.POST("/clients/claude/launch", h.launchClaude)
	group.POST("/clients/codex/configure", h.configureCodex)
	group.POST("/clients/codex/launch", h.launchCodex)
}

func (h *Handler) session(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"data": gin.H{"api_key": h.key}})
}

func (h *Handler) candidates(c *gin.Context) {
	model := strings.TrimSpace(c.Query("model"))
	strategy := h.routeStrategy(c)
	channels, err := h.router.Candidates(c.Request.Context(), model, strategy)
	if err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": sanitizeChannels(channels)})
}

type channelRequest struct {
	Name            string   `json:"name" binding:"required"`
	Provider        string   `json:"provider" binding:"required"`
	APIMode         string   `json:"api_mode"`
	Endpoint        string   `json:"endpoint" binding:"required"`
	APIKey          string   `json:"api_key"`
	PrimaryModel    string   `json:"primary_model" binding:"required"`
	ExtraModels     []string `json:"extra_models"`
	Enabled         *bool    `json:"enabled"`
	IntervalSeconds *int     `json:"interval_seconds"`
}

func (h *Handler) list(c *gin.Context) {
	channels, err := h.store.List(c.Request.Context())
	if err != nil {
		writeError(c, err)
		return
	}
	if c.Query("include_secrets") != "1" && c.Query("include_secrets") != "true" {
		channels = sanitizeChannels(channels)
	}
	c.JSON(http.StatusOK, gin.H{"data": channels})
}

func (h *Handler) get(c *gin.Context) {
	id, ok := channelID(c)
	if !ok {
		return
	}
	ch, err := h.store.Get(c.Request.Context(), id)
	if err != nil {
		writeError(c, err)
		return
	}
	if c.Query("include_secrets") == "1" || c.Query("include_secrets") == "true" {
		c.JSON(http.StatusOK, gin.H{"data": ch})
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": sanitizeChannel(ch)})
}

func (h *Handler) create(c *gin.Context) {
	var input channelRequest
	if err := c.ShouldBindJSON(&input); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	ch, err := requestToChannel(input)
	if err != nil {
		writeError(c, err)
		return
	}
	if err := h.store.Save(c.Request.Context(), ch); err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusCreated, gin.H{"data": sanitizeChannel(ch)})
}

func (h *Handler) update(c *gin.Context) {
	id, ok := channelID(c)
	if !ok {
		return
	}
	var input channelRequest
	if err := c.ShouldBindJSON(&input); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	existing, err := h.store.Get(c.Request.Context(), id)
	if err != nil {
		writeError(c, err)
		return
	}
	if strings.TrimSpace(input.APIKey) == "" || strings.Contains(input.APIKey, "***") {
		input.APIKey = existing.APIKey
	}
	ch, err := requestToChannel(input)
	if err != nil {
		writeError(c, err)
		return
	}
	// Apply only the configuration fields to whatever is in the database now.
	// A probe or forward that finished while this handler was running keeps its
	// result instead of being overwritten by the runtime state read above.
	var updated *channel.Channel
	if err := h.store.UpdateRuntime(c.Request.Context(), id, func(fresh *channel.Channel) {
		// Changing the cadence is an explicit instruction: drop the adaptive
		// backoff so the new interval takes effect immediately instead of the
		// channel staying on its escalated one.
		if fresh.IntervalSeconds != ch.IntervalSeconds {
			fresh.ProbeStreak = 0
		}
		if fresh.PrimaryModel != ch.PrimaryModel {
			fresh.ProbeStreak = 0
		}
		channel.CopyConfigState(fresh, ch)
		updated = fresh
	}); err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": sanitizeChannel(updated)})
}

func (h *Handler) remove(c *gin.Context) {
	id, ok := channelID(c)
	if !ok {
		return
	}
	if err := h.store.Delete(c.Request.Context(), id); err != nil {
		writeError(c, err)
		return
	}
	c.Status(http.StatusNoContent)
}

// injectionCheck measures the channel's server-side prompt injection: it sends
// a one-token message, compares the upstream-reported prompt tokens against
// normal template overhead, and asks the model to reveal injected content when
// inflated. By default only the primary model is checked; ?all=1 checks every
// model the channel lists, which shows whether a station injects per-model.
// The primary model's result is persisted for the channel list column.
func (h *Handler) injectionCheck(c *gin.Context) {
	id, ok := channelID(c)
	if !ok {
		return
	}
	ch, err := h.store.Get(c.Request.Context(), id)
	if err != nil {
		writeError(c, err)
		return
	}
	if h.runner == nil || !h.runner.AcquireChannel(id) {
		c.JSON(http.StatusConflict, gin.H{"code": "probe_busy", "error": channel.ErrProbeBusy.Error()})
		return
	}
	defer h.runner.ReleaseChannel(id)

	targetModel := strings.TrimSpace(c.Query("model"))
	checkAll := c.Query("all") == "1" || c.Query("all") == "true"

	var models []string
	if targetModel != "" {
		models = []string{targetModel}
	} else if checkAll {
		models = append([]string{ch.PrimaryModel}, ch.ExtraModels...)
	} else {
		models = []string{ch.PrimaryModel}
	}

	timeout := 1 * time.Minute
	if checkAll {
		timeout = 8 * time.Minute
	}
	checkCtx, cancel := context.WithTimeout(c.Request.Context(), timeout)
	defer cancel()

	reports := probe.CheckInjectionModels(checkCtx, probe.Config{
		Provider: ch.Provider,
		APIMode:  ch.APIMode,
		Endpoint: ch.Endpoint,
		APIKey:   ch.APIKey,
	}, models)

	// The channel column reflects the primary model, the one real traffic uses
	// first; per-model results are returned live rather than persisted.
	for _, report := range reports {
		if report.Model != ch.PrimaryModel {
			continue
		}
		if err := h.store.SetInjection(c.Request.Context(), id, report.State, report.PromptTokens, report.Content); err != nil {
			writeError(c, err)
			return
		}
		break
	}
	c.JSON(http.StatusOK, gin.H{"data": gin.H{"results": reports}})
}

func (h *Handler) probe(c *gin.Context) {
	id, ok := channelID(c)
	if !ok {
		return
	}
	targetModel := strings.TrimSpace(c.Query("model"))
	checkAll := c.Query("all") == "1" || c.Query("all") == "true"

	var results []*probe.Result
	var err error
	switch {
	case targetModel != "":
		results, err = h.runner.ProbeModelNow(c.Request.Context(), id, targetModel)
	case checkAll:
		results, err = h.runner.ProbeAllModelsNow(c.Request.Context(), id)
	default:
		results, err = h.runner.ProbeNow(c.Request.Context(), id)
	}
	if err != nil {
		if errors.Is(err, channel.ErrProbeBusy) {
			c.JSON(http.StatusConflict, gin.H{"code": "probe_busy", "error": err.Error()})
			return
		}
		if errors.Is(err, channel.ErrChannelDisabled) {
			c.JSON(http.StatusConflict, gin.H{"code": "channel_disabled", "error": err.Error()})
			return
		}
		writeError(c, err)
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": results})
}

func (h *Handler) models(c *gin.Context) {
	channels, err := h.store.List(c.Request.Context())
	if err != nil {
		writeError(c, err)
		return
	}
	type modelEntry struct {
		Name     string   `json:"name"`
		Channels []string `json:"channels"`
	}
	entries := make(map[string]*modelEntry)
	hasEnabled := false
	for _, ch := range channels {
		if !ch.Enabled {
			continue
		}
		hasEnabled = true
		for _, model := range append([]string{ch.PrimaryModel}, ch.ExtraModels...) {
			model = strings.TrimSpace(model)
			if model == "" {
				continue
			}
			entry := entries[model]
			if entry == nil {
				entry = &modelEntry{Name: model}
				entries[model] = entry
			}
			entry.Channels = append(entry.Channels, ch.Name)
		}
	}
	data := make([]*modelEntry, 0, len(entries))
	if hasEnabled {
		data = append(data, &modelEntry{Name: "Auto", Channels: []string{"HeiGate 自动路由"}})
	}
	for _, entry := range entries {
		data = append(data, entry)
	}
	sort.Slice(data, func(i, j int) bool {
		return data[i].Name < data[j].Name
	})
	c.JSON(http.StatusOK, gin.H{"data": data})
}

func (h *Handler) getCustomModelMappings(c *gin.Context) {
	mappings := channel.GetCustomModelMappings()
	c.JSON(http.StatusOK, gin.H{"data": mappings})
}

func (h *Handler) setCustomModelMapping(c *gin.Context) {
	var req struct {
		RawModel    string `json:"raw_model" binding:"required"`
		TargetModel string `json:"target_model" binding:"required"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "参数错误: raw_model 与 target_model 为必填项"})
		return
	}
	raw := strings.TrimSpace(req.RawModel)
	target := strings.TrimSpace(req.TargetModel)
	if raw == "" || target == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "模型名称不能为空"})
		return
	}

	mappings := channel.GetCustomModelMappings()
	mappings[strings.ToLower(raw)] = target
	channel.SetCustomModelMappings(mappings)

	if h.store != nil {
		rawBytes, _ := json.Marshal(mappings)
		_ = h.store.SetSetting(c.Request.Context(), "custom_model_mappings", string(rawBytes))
	}

	c.JSON(http.StatusOK, gin.H{"status": "ok", "data": mappings})
}

func (h *Handler) deleteCustomModelMapping(c *gin.Context) {
	raw := strings.TrimSpace(c.Query("raw_model"))
	if raw == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "缺少 raw_model 参数"})
		return
	}

	mappings := channel.GetCustomModelMappings()
	delete(mappings, strings.ToLower(raw))
	channel.SetCustomModelMappings(mappings)

	if h.store != nil {
		rawBytes, _ := json.Marshal(mappings)
		_ = h.store.SetSetting(c.Request.Context(), "custom_model_mappings", string(rawBytes))
	}

	c.JSON(http.StatusOK, gin.H{"status": "ok", "data": mappings})
}

func requestToChannel(input channelRequest) (*channel.Channel, error) {
	enabled := true
	if input.Enabled != nil {
		enabled = *input.Enabled
	}
	apiMode := input.APIMode
	if apiMode == "" {
		apiMode = probe.APIModeChatCompletions
	}
	endpoint := strings.TrimRight(strings.TrimSpace(input.Endpoint), "/")
	if !strings.HasPrefix(strings.ToLower(endpoint), "http://") && !strings.HasPrefix(strings.ToLower(endpoint), "https://") {
		endpoint = "https://" + endpoint
	}
	ch := &channel.Channel{
		Name: input.Name, Provider: input.Provider, APIMode: apiMode,
		Endpoint: endpoint, APIKey: input.APIKey,
		PrimaryModel: input.PrimaryModel, ExtraModels: input.ExtraModels, Enabled: enabled,
	}
	// Automatic probes are disabled globally. Keep the persisted value at zero
	// so old clients cannot accidentally re-enable quota-consuming probes.
	ch.IntervalSeconds = 0
	if err := probe.ValidateInterval(ch.IntervalSeconds); err != nil {
		return nil, err
	}
	if err := probe.ValidateConfig(probe.Config{
		Provider: ch.Provider, APIMode: ch.APIMode, Endpoint: ch.Endpoint,
		APIKey: ch.APIKey, PrimaryModel: ch.PrimaryModel,
	}); err != nil {
		return nil, err
	}
	return ch, nil
}

func sanitizeChannels(channels []*channel.Channel) []*channel.Channel {
	for index, ch := range channels {
		channels[index] = sanitizeChannel(ch)
	}
	return channels
}

func sanitizeChannel(ch *channel.Channel) *channel.Channel {
	if ch == nil {
		return nil
	}
	copy := *ch
	if len(copy.APIKey) > 4 {
		copy.APIKey = copy.APIKey[:4] + "***"
	} else {
		copy.APIKey = "***"
	}
	return &copy
}

func channelID(c *gin.Context) (int64, bool) {
	id, err := strconv.ParseInt(c.Param("id"), 10, 64)
	if err != nil || id <= 0 {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid channel id"})
		return 0, false
	}
	return id, true
}

func writeError(c *gin.Context, err error) {
	status := http.StatusInternalServerError
	if strings.Contains(strings.ToLower(err.Error()), "not found") || strings.Contains(strings.ToLower(err.Error()), "invalid") || strings.Contains(strings.ToLower(err.Error()), "required") {
		if strings.Contains(strings.ToLower(err.Error()), "not found") {
			status = http.StatusNotFound
		} else {
			status = http.StatusBadRequest
		}
	}
	c.JSON(status, gin.H{"error": err.Error()})
}

func (h *Handler) duplicate(c *gin.Context) {
	id, ok := channelID(c)
	if !ok {
		return
	}
	existing, err := h.store.Get(c.Request.Context(), id)
	if err != nil {
		writeError(c, err)
		return
	}
	dup := *existing
	dup.ID = 0
	dup.Name = fmt.Sprintf("%s (副本)", existing.Name)
	dup.ModelHealth = nil
	dup.CreatedAt = time.Time{}
	dup.UpdatedAt = time.Time{}
	if err := h.store.Save(c.Request.Context(), &dup); err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusCreated, gin.H{"data": sanitizeChannel(&dup)})
}

func (h *Handler) getLogs(c *gin.Context) {
	limit := 100
	if l := c.Query("limit"); l != "" {
		if val, err := strconv.Atoi(l); err == nil && val > 0 {
			limit = val
		}
	}
	logs, err := h.store.ListLogs(c.Request.Context(), limit)
	if err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": logs})
}

func (h *Handler) streamLogs(c *gin.Context) {
	if h.store == nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "store not initialized"})
		return
	}
	c.Writer.Header().Set("Content-Type", "text/event-stream")
	c.Writer.Header().Set("Cache-Control", "no-cache")
	c.Writer.Header().Set("Connection", "keep-alive")
	c.Writer.Header().Set("X-Accel-Buffering", "no")

	flusher, ok := c.Writer.(http.Flusher)
	if !ok {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "streaming unsupported"})
		return
	}
	flusher.Flush()

	ch, unsubscribe := h.store.SubscribeLogs()
	defer unsubscribe()

	_, _ = fmt.Fprintf(c.Writer, "event: connected\ndata: {\"connected\":true}\n\n")
	flusher.Flush()

	ticker := time.NewTicker(15 * time.Second)
	defer ticker.Stop()

	ctx := c.Request.Context()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			_, _ = fmt.Fprintf(c.Writer, "event: ping\ndata: {\"ts\":%d}\n\n", time.Now().UnixMilli())
			flusher.Flush()
		case evt, ok := <-ch:
			if !ok {
				return
			}
			payload, err := json.Marshal(evt.Log)
			if err != nil {
				continue
			}
			switch evt.Type {
			case "created":
				_, _ = fmt.Fprintf(c.Writer, "event: log_created\ndata: %s\n\n", payload)
			case "updated":
				_, _ = fmt.Fprintf(c.Writer, "event: log_updated\ndata: %s\n\n", payload)
			case "cleared":
				_, _ = fmt.Fprintf(c.Writer, "event: logs_cleared\ndata: {}\n\n")
			case "pruned":
				_, _ = fmt.Fprintf(c.Writer, "event: logs_pruned\ndata: {}\n\n")
			}
			flusher.Flush()
		}
	}
}

func (h *Handler) clearLogs(c *gin.Context) {
	if err := h.store.ClearLogs(c.Request.Context()); err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusOK, gin.H{"status": "ok", "message": "logs cleared"})
}

func (h *Handler) pruneLogs(c *gin.Context) {
	var input struct {
		Days     *int `json:"days"`
		MaxCount *int `json:"max_count"`
	}
	_ = c.ShouldBindJSON(&input)
	var deleted int64
	var err error
	if input.Days != nil || input.MaxCount != nil {
		if input.Days != nil && *input.Days > 0 {
			del, e := h.store.PruneLogsOlderThan(c.Request.Context(), *input.Days)
			if e == nil {
				deleted += del
			}
		}
		if input.MaxCount != nil && *input.MaxCount > 0 {
			del, e := h.store.PruneLogsLimit(c.Request.Context(), *input.MaxCount)
			if e == nil {
				deleted += del
			}
		}
	} else {
		deleted, err = h.store.AutoPruneLogs(c.Request.Context())
	}
	if err != nil {
		writeError(c, err)
		return
	}
	logsCount, _ := h.store.CountLogs(c.Request.Context())
	c.JSON(http.StatusOK, gin.H{"status": "ok", "deleted": deleted, "logs_count": logsCount})
}

func (h *Handler) analytics(c *gin.Context) {
	days := 7
	if raw := strings.TrimSpace(c.Query("days")); raw != "" {
		if parsed, err := strconv.Atoi(raw); err == nil && parsed >= 0 {
			days = parsed
		}
	}
	result, err := h.store.GetAnalytics(c.Request.Context(), days)
	if err != nil {
		writeError(c, err)
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": result})
}

func (h *Handler) systemInfo(c *gin.Context) {
	retentionDaysStr, _ := h.store.GetSetting(c.Request.Context(), "log_retention_days", "7")
	retentionDays, _ := strconv.Atoi(retentionDaysStr)
	if retentionDays <= 0 && retentionDaysStr != "0" {
		retentionDays = 7
	}
	maxCountStr, _ := h.store.GetSetting(c.Request.Context(), "log_max_count", "5000")
	maxCount, _ := strconv.Atoi(maxCountStr)
	if maxCount <= 0 && maxCountStr != "0" {
		maxCount = 5000
	}
	logsCount, _ := h.store.CountLogs(c.Request.Context())
	routingStrategy := h.routeStrategy(c)
	dataDir := datadir.Path()
	dbPath := datadir.DatabasePath()
	var dbSize int64
	if stat, err := os.Stat(dbPath); err == nil { //nolint:gosec // dbPath is local-only and is never accepted from a request.
		dbSize = stat.Size()
	}
	c.JSON(http.StatusOK, gin.H{"data": gin.H{
		"data_dir":           dataDir,
		"db_path":            dbPath,
		"db_size_bytes":      dbSize,
		"log_retention_days": retentionDays,
		"log_max_count":      maxCount,
		"logs_count":         logsCount,
		"routing_strategy":   routingStrategy,
	}})
}

func (h *Handler) updateSettings(c *gin.Context) {
	var input struct {
		LogRetentionDays *int    `json:"log_retention_days"`
		LogMaxCount      *int    `json:"log_max_count"`
		RoutingStrategy  *string `json:"routing_strategy"`
	}
	if err := c.ShouldBindJSON(&input); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	settingsChanged := false
	if input.LogRetentionDays != nil {
		val := strconv.Itoa(*input.LogRetentionDays)
		if err := h.store.SetSetting(c.Request.Context(), "log_retention_days", val); err != nil {
			writeError(c, err)
			return
		}
		settingsChanged = true
	}
	if input.LogMaxCount != nil {
		val := strconv.Itoa(*input.LogMaxCount)
		if err := h.store.SetSetting(c.Request.Context(), "log_max_count", val); err != nil {
			writeError(c, err)
			return
		}
		settingsChanged = true
	}
	if settingsChanged {
		_, _ = h.store.AutoPruneLogs(c.Request.Context())
	}
	if input.RoutingStrategy != nil {
		strategy := strings.TrimSpace(*input.RoutingStrategy)
		if strategy != channel.RouteSmartQuality && strategy != channel.RouteRoundRobin && strategy != channel.RouteLatency {
			c.JSON(http.StatusBadRequest, gin.H{"error": "invalid routing strategy"})
			return
		}
		if err := h.store.SetSetting(c.Request.Context(), "routing_strategy", strategy); err != nil {
			writeError(c, err)
			return
		}
	}
	c.JSON(http.StatusOK, gin.H{"status": "ok"})
}

func (h *Handler) routeStrategy(c *gin.Context) string {
	if strategy := strings.TrimSpace(c.Query("strategy")); strategy != "" {
		return strategy
	}
	value, err := h.store.GetSetting(c.Request.Context(), "routing_strategy", channel.RouteLatency)
	if err != nil {
		return channel.RouteLatency
	}
	if value != channel.RouteLatency && value != channel.RouteSmartQuality && value != channel.RouteRoundRobin {
		return channel.RouteLatency
	}
	return value
}

type upstreamModelsRequest struct {
	Endpoint string `json:"endpoint" binding:"required"`
	APIKey   string `json:"api_key"`
	Provider string `json:"provider"`
}

func (h *Handler) fetchUpstreamModels(c *gin.Context) {
	var req upstreamModelsRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "缺少必要参数 Base URL"})
		return
	}

	endpoint := strings.TrimSpace(req.Endpoint)
	if endpoint == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "Base URL 不能为空"})
		return
	}

	if !strings.HasPrefix(endpoint, "http://") && !strings.HasPrefix(endpoint, "https://") {
		endpoint = "https://" + endpoint
	}
	endpoint = strings.TrimRight(endpoint, "/")

	var candidateURLs []string
	if strings.HasSuffix(endpoint, "/models") {
		candidateURLs = append(candidateURLs, endpoint)
	} else {
		candidateURLs = append(candidateURLs, endpoint+"/models")
		if !strings.HasSuffix(endpoint, "/v1") {
			candidateURLs = append(candidateURLs, endpoint+"/v1/models")
		}
	}

	client := &http.Client{
		Timeout: 12 * time.Second,
	}

	var lastErr error
	var models []string

	for _, targetURL := range candidateURLs {
		httpReq, err := http.NewRequestWithContext(c.Request.Context(), http.MethodGet, targetURL, nil)
		if err != nil {
			lastErr = err
			continue
		}

		httpReq.Header.Set("Accept", "application/json")
		httpReq.Header.Set("User-Agent", "HeiGate/1.0")

		apiKey := strings.TrimSpace(req.APIKey)
		if apiKey != "" && !strings.Contains(apiKey, "***") {
			httpReq.Header.Set("Authorization", "Bearer "+apiKey)
			httpReq.Header.Set("x-api-key", apiKey)
		}
		if req.Provider == "anthropic" {
			httpReq.Header.Set("anthropic-version", "2023-06-01")
		}

		resp, err := client.Do(httpReq)
		if err != nil {
			lastErr = err
			continue
		}

		bodyBytes, _ := io.ReadAll(io.LimitReader(resp.Body, 1024*1024))
		_ = resp.Body.Close()

		if resp.StatusCode != http.StatusOK {
			var errPayload struct {
				Error struct {
					Message string `json:"message"`
				} `json:"error"`
				Message string `json:"message"`
			}
			errMsg := ""
			if json.Unmarshal(bodyBytes, &errPayload) == nil {
				if errPayload.Error.Message != "" {
					errMsg = errPayload.Error.Message
				} else if errPayload.Message != "" {
					errMsg = errPayload.Message
				}
			}
			if errMsg != "" {
				lastErr = fmt.Errorf("上游提示: %s (HTTP %d)", errMsg, resp.StatusCode)
			} else {
				lastErr = fmt.Errorf("上游返回 HTTP %d", resp.StatusCode)
			}
			if resp.StatusCode == http.StatusNotFound && len(candidateURLs) > 1 {
				continue
			}
			break
		}

		var payload struct {
			Data   []any `json:"data"`
			Models []any `json:"models"`
		}

		if err := json.Unmarshal(bodyBytes, &payload); err == nil {
			items := payload.Data
			if len(items) == 0 {
				items = payload.Models
			}
			seen := make(map[string]struct{})
			for _, item := range items {
				switch val := item.(type) {
				case string:
					trimmed := strings.TrimSpace(val)
					if trimmed != "" {
						if _, exists := seen[trimmed]; !exists {
							seen[trimmed] = struct{}{}
							models = append(models, trimmed)
						}
					}
				case map[string]any:
					name := ""
					if id, ok := val["id"].(string); ok && id != "" {
						name = id
					} else if n, ok := val["name"].(string); ok && n != "" {
						name = n
					}
					name = strings.TrimSpace(name)
					if name != "" {
						if _, exists := seen[name]; !exists {
							seen[name] = struct{}{}
							models = append(models, name)
						}
					}
				}
			}
		} else {
			var directList []string
			if err := json.Unmarshal(bodyBytes, &directList); err == nil {
				for _, m := range directList {
					trimmed := strings.TrimSpace(m)
					if trimmed != "" {
						models = append(models, trimmed)
					}
				}
			}
		}

		if len(models) > 0 {
			lastErr = nil
			break
		} else {
			lastErr = fmt.Errorf("上游响应成功，但未解析出任何模型 ID")
		}
	}

	if lastErr != nil {
		c.JSON(http.StatusBadGateway, gin.H{"error": lastErr.Error()})
		return
	}

	c.JSON(http.StatusOK, gin.H{"data": models})
}
