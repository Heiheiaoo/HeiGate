package channel

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"math"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/Heiheiaoo/heigate/internal/probe"
	_ "modernc.org/sqlite"
)

// Channel is the local-only channel record used by the desktop gateway.
// It deliberately has no user, group, billing, or cloud-sync fields.
type Channel struct {
	ID                 int64      `json:"id"`
	Name               string     `json:"name"`
	Provider           string     `json:"provider"`
	APIMode            string     `json:"api_mode"`
	Endpoint           string     `json:"endpoint"`
	APIKey             string     `json:"api_key"`
	PrimaryModel       string     `json:"primary_model"`
	ExtraModels        []string   `json:"extra_models"`
	Enabled            bool       `json:"enabled"`
	IntervalSeconds    int        `json:"-"` // legacy database column; automatic probes are disabled
	LastStatus         string     `json:"last_status"`
	LastLatencyMs      *int       `json:"last_latency_ms"`
	LastTTFTMs         *int       `json:"last_ttft_ms"`
	LastPingLatencyMs  *int       `json:"last_ping_latency_ms"`
	LastError          string     `json:"last_error"`
	LastCheckedAt      *time.Time `json:"last_checked_at"`
	ProbeStreak        int        `json:"-"` // legacy database column
	FailureCount       int        `json:"failure_count"`
	CircuitState       string     `json:"circuit_state"`
	CooldownUntil      *time.Time `json:"cooldown_until"`
	InjectionState     string     `json:"injection_state"`
	InjectionTokens    *int       `json:"injection_tokens"`
	InjectionContent   string     `json:"injection_content"`
	InjectionCheckedAt *time.Time `json:"injection_checked_at"`
	CreatedAt          time.Time  `json:"created_at"`
	UpdatedAt          time.Time  `json:"updated_at"`

	// EffectiveIntervalSeconds is derived, not stored: it is the interval the
	// scheduler will actually use after adaptive backoff, so the UI can show
	// the real cadence instead of the configured one.
	EffectiveIntervalSeconds int           `json:"-"` // legacy derived field
	ModelHealth              []ModelHealth `json:"model_health,omitempty"`
}

type ModelHealth struct {
	Model     string     `json:"model"`
	Status    string     `json:"status"`
	LatencyMs *int       `json:"latency_ms,omitempty"`
	TTFTMs    *int       `json:"ttft_ms,omitempty"`
	Error     string     `json:"error,omitempty"`
	CheckedAt *time.Time `json:"checked_at,omitempty"`
}

// LogEvent represents a real-time event for desktop request logs.
type LogEvent struct {
	Type string      `json:"type"` // "created", "updated", "cleared", "pruned"
	Log  *RequestLog `json:"log,omitempty"`
}

// Store persists desktop channels in a single local SQLite file.
// The database handle is configured for one writer because desktop workloads are
// small and serialized writes make probe updates deterministic.
type Store struct {
	db             *sql.DB
	runtimeMu      sync.Mutex
	logMu          sync.RWMutex
	logSubscribers map[chan LogEvent]struct{}
}

const schemaSQL = `
CREATE TABLE IF NOT EXISTS desktop_channels (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  provider TEXT NOT NULL,
  api_mode TEXT NOT NULL DEFAULT 'chat_completions',
  endpoint TEXT NOT NULL,
  api_key TEXT NOT NULL,
  primary_model TEXT NOT NULL,
  extra_models_json TEXT NOT NULL DEFAULT '[]',
  enabled INTEGER NOT NULL DEFAULT 1,
  interval_seconds INTEGER NOT NULL DEFAULT 300,
  last_status TEXT NOT NULL DEFAULT 'unknown',
  last_latency_ms INTEGER,
  last_ttft_ms INTEGER,
  last_ping_latency_ms INTEGER,
  last_error TEXT NOT NULL DEFAULT '',
  last_checked_at TEXT,
  probe_streak INTEGER NOT NULL DEFAULT 0,
  failure_count INTEGER NOT NULL DEFAULT 0,
  circuit_state TEXT NOT NULL DEFAULT 'closed',
  cooldown_until TEXT,
  injection_state TEXT NOT NULL DEFAULT 'unknown',
  injection_tokens INTEGER,
  injection_content TEXT NOT NULL DEFAULT '',
  injection_checked_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_desktop_channels_enabled ON desktop_channels(enabled);

CREATE TABLE IF NOT EXISTS desktop_request_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  model TEXT NOT NULL,
  channel_name TEXT NOT NULL,
  status_code INTEGER NOT NULL,
  latency_ms INTEGER NOT NULL,
  ttft_ms INTEGER NOT NULL DEFAULT 0,
  prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  is_stream INTEGER NOT NULL DEFAULT 0,
  error_message TEXT NOT NULL DEFAULT '',
  failover_from TEXT NOT NULL DEFAULT '',
  is_failover INTEGER NOT NULL DEFAULT 0,
  upstream_model TEXT NOT NULL DEFAULT '',
  endpoint TEXT NOT NULL DEFAULT '',
  client_ip TEXT NOT NULL DEFAULT '',
  user_agent TEXT NOT NULL DEFAULT '',
  request_path TEXT NOT NULL DEFAULT '',
  finish_reason TEXT NOT NULL DEFAULT '',
  trace_json TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_desktop_request_logs_created_at ON desktop_request_logs(created_at);

CREATE TABLE IF NOT EXISTS desktop_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS desktop_model_health (
  channel_id INTEGER NOT NULL,
  model TEXT NOT NULL,
  status TEXT NOT NULL,
  latency_ms INTEGER,
  ttft_ms INTEGER,
  error TEXT NOT NULL DEFAULT '',
  checked_at TEXT,
  PRIMARY KEY (channel_id, model),
  FOREIGN KEY (channel_id) REFERENCES desktop_channels(id) ON DELETE CASCADE
);
`

// OpenStore opens or creates a local SQLite database.
func OpenStore(path string) (*Store, error) {
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open desktop channel database: %w", err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	store := &Store{db: db}
	if err := store.initialize(context.Background()); err != nil {
		_ = db.Close()
		return nil, err
	}
	return store, nil
}

// NewStore wraps an already opened SQLite database. It is useful
// for tests and for a desktop host that owns the database lifecycle.
func NewStore(db *sql.DB) (*Store, error) {
	if db == nil {
		return nil, fmt.Errorf("desktop channel database is nil")
	}
	store := &Store{db: db}
	if err := store.initialize(context.Background()); err != nil {
		return nil, err
	}
	return store, nil
}

func (s *Store) initialize(ctx context.Context) error {
	if _, err := s.db.ExecContext(ctx, "PRAGMA foreign_keys = ON"); err != nil {
		return fmt.Errorf("enable desktop sqlite foreign keys: %w", err)
	}
	if _, err := s.db.ExecContext(ctx, schemaSQL); err != nil {
		return fmt.Errorf("create desktop channel schema: %w", err)
	}
	// Keep existing desktop databases forward-compatible with the resilience,
	// token metrics, and rich trace audit fields added after the initial desktop release.
	for _, statement := range []string{
		"ALTER TABLE desktop_channels ADD COLUMN failure_count INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_channels ADD COLUMN circuit_state TEXT NOT NULL DEFAULT 'closed'",
		"ALTER TABLE desktop_channels ADD COLUMN cooldown_until TEXT",
		"ALTER TABLE desktop_channels ADD COLUMN last_ttft_ms INTEGER",
		"ALTER TABLE desktop_channels ADD COLUMN injection_state TEXT NOT NULL DEFAULT 'unknown'",
		"ALTER TABLE desktop_channels ADD COLUMN injection_tokens INTEGER",
		"ALTER TABLE desktop_channels ADD COLUMN injection_content TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_channels ADD COLUMN injection_checked_at TEXT",
		"ALTER TABLE desktop_channels ADD COLUMN probe_streak INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_request_logs ADD COLUMN ttft_ms INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_request_logs ADD COLUMN prompt_tokens INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_request_logs ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_request_logs ADD COLUMN total_tokens INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_request_logs ADD COLUMN is_stream INTEGER NOT NULL DEFAULT 0",
		"ALTER TABLE desktop_request_logs ADD COLUMN upstream_model TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_request_logs ADD COLUMN endpoint TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_request_logs ADD COLUMN client_ip TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_request_logs ADD COLUMN user_agent TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_request_logs ADD COLUMN request_path TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_request_logs ADD COLUMN finish_reason TEXT NOT NULL DEFAULT ''",
		"ALTER TABLE desktop_request_logs ADD COLUMN trace_json TEXT NOT NULL DEFAULT ''",
	} {
		if _, err := s.db.ExecContext(ctx, statement); err != nil && !strings.Contains(strings.ToLower(err.Error()), "duplicate column") {
			return fmt.Errorf("migrate desktop channel schema: %w", err)
		}
	}

	// Channel priority was never exposed in the UI and no routing strategy read
	// it, so the column is dropped from databases created before its removal.
	// Older SQLite builds without DROP COLUMN support leave the column in place;
	// it is simply ignored because no query references it.
	_, _ = s.db.ExecContext(ctx, "ALTER TABLE desktop_channels DROP COLUMN priority")

	// Auto-repair legacy false-positive 502 logs where streaming completed tokens but was flagged context canceled
	_, _ = s.db.ExecContext(ctx, `
UPDATE desktop_request_logs
SET status_code = 200,
    finish_reason = 'stop',
    error_message = '流式生成完成 (客户端在接收后断开连接)'
WHERE status_code = 502
  AND completion_tokens > 0
  AND error_message LIKE '%context canceled%'`)
	if err := s.pruneAllModelHealth(ctx); err != nil {
		return fmt.Errorf("prune stale model health: %w", err)
	}

	return nil
}

func (s *Store) pruneAllModelHealth(ctx context.Context) error {
	rows, err := s.db.QueryContext(ctx, `SELECT id, primary_model, extra_models_json FROM desktop_channels`)
	if err != nil {
		return err
	}
	type channelModels struct {
		id      int64
		primary string
		extra   string
	}
	var channels []channelModels
	for rows.Next() {
		var item channelModels
		if err := rows.Scan(&item.id, &item.primary, &item.extra); err != nil {
			_ = rows.Close()
			return err
		}
		channels = append(channels, item)
	}
	if err := rows.Close(); err != nil {
		return err
	}
	if err := rows.Err(); err != nil {
		return err
	}
	for _, item := range channels {
		var extra []string
		if err := json.Unmarshal([]byte(item.extra), &extra); err != nil {
			return err
		}
		if err := s.PruneModelHealth(ctx, item.id, append([]string{item.primary}, extra...)); err != nil {
			return err
		}
	}
	return nil
}

func (s *Store) Close() error {
	if s == nil || s.db == nil {
		return nil
	}
	return s.db.Close()
}

func (s *Store) List(ctx context.Context) ([]*Channel, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT id, name, provider, api_mode, endpoint, api_key, primary_model,
       extra_models_json, enabled, interval_seconds, last_status,
       last_latency_ms, last_ttft_ms, last_ping_latency_ms, last_error, last_checked_at,
       probe_streak, failure_count, circuit_state, cooldown_until,
       injection_state, injection_tokens, injection_content, injection_checked_at,
       created_at, updated_at
FROM desktop_channels ORDER BY id ASC`)
	if err != nil {
		return nil, fmt.Errorf("list desktop channels: %w", err)
	}
	defer func() { _ = rows.Close() }()

	channels := make([]*Channel, 0)
	for rows.Next() {
		channel, err := scanChannel(rows)
		if err != nil {
			return nil, err
		}
		channels = append(channels, channel)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate desktop channels: %w", err)
	}
	if err := rows.Close(); err != nil {
		return nil, err
	}
	for _, channel := range channels {
		if err := s.loadModelHealth(ctx, channel); err != nil {
			return nil, err
		}
	}
	return channels, nil
}

func (s *Store) Get(ctx context.Context, id int64) (*Channel, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT id, name, provider, api_mode, endpoint, api_key, primary_model,
       extra_models_json, enabled, interval_seconds, last_status,
       last_latency_ms, last_ttft_ms, last_ping_latency_ms, last_error, last_checked_at,
       probe_streak, failure_count, circuit_state, cooldown_until,
       injection_state, injection_tokens, injection_content, injection_checked_at,
       created_at, updated_at
FROM desktop_channels WHERE id = ?`, id)
	channel, err := scanChannel(row)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, fmt.Errorf("desktop channel %d not found", id)
		}
		return nil, err
	}
	if err := s.loadModelHealth(ctx, channel); err != nil {
		return nil, err
	}
	return channel, nil
}

func (s *Store) loadModelHealth(ctx context.Context, ch *Channel) error {
	rows, err := s.db.QueryContext(ctx, `SELECT model, status, latency_ms, ttft_ms, error, checked_at FROM desktop_model_health WHERE channel_id = ? ORDER BY model`, ch.ID)
	if err != nil {
		return fmt.Errorf("list model health: %w", err)
	}
	defer rows.Close()
	for rows.Next() {
		var h ModelHealth
		var latency, ttft sql.NullInt64
		var checked sql.NullString
		if err := rows.Scan(&h.Model, &h.Status, &latency, &ttft, &h.Error, &checked); err != nil {
			return err
		}
		if latency.Valid {
			v := int(latency.Int64)
			h.LatencyMs = &v
		}
		if ttft.Valid {
			v := int(ttft.Int64)
			h.TTFTMs = &v
		}
		h.CheckedAt = parseTime(checked)
		ch.ModelHealth = append(ch.ModelHealth, h)
	}
	return rows.Err()
}

func (s *Store) SetModelHealth(ctx context.Context, channelID int64, health ModelHealth) error {
	_, err := s.db.ExecContext(ctx, `INSERT INTO desktop_model_health(channel_id, model, status, latency_ms, ttft_ms, error, checked_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(channel_id, model) DO UPDATE SET status=excluded.status, latency_ms=excluded.latency_ms, ttft_ms=excluded.ttft_ms, error=excluded.error, checked_at=excluded.checked_at`, channelID, health.Model, health.Status, nullableInt(health.LatencyMs), nullableInt(health.TTFTMs), health.Error, nullableTime(health.CheckedAt))
	return err
}

func (s *Store) Save(ctx context.Context, channel *Channel) error {
	if channel == nil {
		return fmt.Errorf("desktop channel is nil")
	}
	extraModels, err := json.Marshal(channel.ExtraModels)
	if err != nil {
		return fmt.Errorf("marshal desktop channel models: %w", err)
	}
	now := time.Now().UTC()
	if channel.CreatedAt.IsZero() {
		channel.CreatedAt = now
	}
	channel.UpdatedAt = now
	if channel.APIMode == "" {
		channel.APIMode = probe.APIModeChatCompletions
	}
	// Zero is a valid manual-only probe interval for the desktop edition.
	if channel.IntervalSeconds < 0 {
		channel.IntervalSeconds = 0
	}
	if channel.LastStatus == "" {
		channel.LastStatus = "unknown"
	}
	if channel.CircuitState == "" {
		channel.CircuitState = "closed"
	}

	args := []any{channel.Name, channel.Provider, channel.APIMode, channel.Endpoint, channel.APIKey,
		channel.PrimaryModel, string(extraModels), channel.Enabled, channel.IntervalSeconds,
		channel.LastStatus, channel.LastLatencyMs, channel.LastTTFTMs, channel.LastPingLatencyMs,
		channel.LastError, nullableTime(channel.LastCheckedAt), channel.ProbeStreak, channel.FailureCount, channel.CircuitState,
		nullableTime(channel.CooldownUntil), channel.InjectionState, nullableInt(channel.InjectionTokens),
		channel.InjectionContent, nullableTime(channel.InjectionCheckedAt),
		channel.CreatedAt.UTC().Format(time.RFC3339Nano), channel.UpdatedAt.UTC().Format(time.RFC3339Nano)}
	if channel.ID == 0 {
		result, err := s.db.ExecContext(ctx, `INSERT INTO desktop_channels
 (name, provider, api_mode, endpoint, api_key, primary_model, extra_models_json,
  enabled, interval_seconds, last_status, last_latency_ms, last_ttft_ms,
  last_ping_latency_ms, last_error, last_checked_at, probe_streak, failure_count, circuit_state,
  cooldown_until, injection_state, injection_tokens, injection_content, injection_checked_at,
  created_at, updated_at)
 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`, args...)
		if err != nil {
			return fmt.Errorf("insert desktop channel: %w", err)
		}
		channel.ID, err = result.LastInsertId()
		channel.EffectiveIntervalSeconds = effectiveIntervalSeconds(channel)
		if err == nil {
			err = s.PruneModelHealth(ctx, channel.ID, append([]string{channel.PrimaryModel}, channel.ExtraModels...))
		}
		return err
	}
	channel.EffectiveIntervalSeconds = effectiveIntervalSeconds(channel)
	args = append(args, channel.ID)
	if _, err := s.db.ExecContext(ctx, `UPDATE desktop_channels SET
 name=?, provider=?, api_mode=?, endpoint=?, api_key=?, primary_model=?, extra_models_json=?,
 enabled=?, interval_seconds=?, last_status=?, last_latency_ms=?, last_ttft_ms=?,
 last_ping_latency_ms=?, last_error=?, last_checked_at=?, probe_streak=?, failure_count=?, circuit_state=?,
 cooldown_until=?, injection_state=?, injection_tokens=?, injection_content=?, injection_checked_at=?,
 created_at=?, updated_at=?
 WHERE id=?`, args...); err != nil {
		return fmt.Errorf("update desktop channel: %w", err)
	}
	return s.PruneModelHealth(ctx, channel.ID, append([]string{channel.PrimaryModel}, channel.ExtraModels...))
}

// PruneModelHealth removes snapshots for models no longer configured on the
// channel, preventing deleted models from reappearing as stale health rows.
func (s *Store) PruneModelHealth(ctx context.Context, channelID int64, models []string) error {
	if channelID <= 0 {
		return nil
	}
	kept := make([]string, 0, len(models))
	seen := make(map[string]struct{})
	for _, model := range models {
		model = strings.TrimSpace(model)
		if model == "" {
			continue
		}
		if _, exists := seen[model]; exists {
			continue
		}
		seen[model] = struct{}{}
		kept = append(kept, model)
	}
	if len(kept) == 0 {
		_, err := s.db.ExecContext(ctx, `DELETE FROM desktop_model_health WHERE channel_id = ?`, channelID)
		return err
	}
	placeholders := strings.TrimRight(strings.Repeat("?,", len(kept)), ",")
	args := make([]any, 0, len(kept)+1)
	args = append(args, channelID)
	for _, model := range kept {
		args = append(args, model)
	}
	_, err := s.db.ExecContext(ctx, `DELETE FROM desktop_model_health WHERE channel_id = ? AND model NOT IN (`+placeholders+`)`, args...)
	return err
}

// effectiveIntervalSeconds is the cadence the scheduler will actually use for
// this channel, after adaptive backoff. Derived on read so the UI can show the
// real interval rather than the configured one.
func effectiveIntervalSeconds(ch *Channel) int {
	return 0
}

// CopyConfigState copies the user-editable configuration fields from src to
// dst, leaving every runtime health field untouched. The update handler uses
// it so an edit cannot clobber a probe or forward result that landed while the
// request was in flight.
func CopyConfigState(dst, src *Channel) {
	dst.Name = src.Name
	dst.Provider = src.Provider
	dst.APIMode = src.APIMode
	dst.Endpoint = src.Endpoint
	dst.APIKey = src.APIKey
	dst.PrimaryModel = src.PrimaryModel
	dst.ExtraModels = src.ExtraModels
	dst.Enabled = src.Enabled
	dst.IntervalSeconds = src.IntervalSeconds
}

// UpdateRuntime re-reads the channel by id, lets mutate adjust it, and saves.
// Probes and forward attempts hold a channel snapshot for tens of seconds; a
// user edit during that window changes configuration fields (models, endpoint)
// in the database, and writing the stale snapshot back wholesale would
// resurrect the old configuration. Callers therefore mutate only runtime
// health fields so configuration always stays fresh. The mutex keeps concurrent
// read-modify-write updates from discarding each other's counters.
func (s *Store) UpdateRuntime(ctx context.Context, id int64, mutate func(*Channel)) error {
	if mutate == nil {
		return fmt.Errorf("runtime channel mutation is nil")
	}
	s.runtimeMu.Lock()
	defer s.runtimeMu.Unlock()
	fresh, err := s.Get(ctx, id)
	if err != nil {
		return err
	}
	mutate(fresh)
	return s.Save(ctx, fresh)
}

// SetInjection persists an injection-check result. A focused UPDATE so it
// never races a concurrent channel edit.
func (s *Store) SetInjection(ctx context.Context, id int64, state string, tokens int, content string) error {
	_, err := s.db.ExecContext(ctx, `
UPDATE desktop_channels
SET injection_state = ?, injection_tokens = ?, injection_content = ?, injection_checked_at = ?, updated_at = ?
WHERE id = ?`,
		state, nullableInt(&tokens), content, time.Now().UTC().Format(time.RFC3339Nano),
		time.Now().UTC().Format(time.RFC3339Nano), id)
	if err != nil {
		return fmt.Errorf("save injection result: %w", err)
	}
	return nil
}

func nullableInt(v *int) any {
	if v == nil {
		return nil
	}
	return *v
}

func (s *Store) Delete(ctx context.Context, id int64) error {
	if _, err := s.db.ExecContext(ctx, "DELETE FROM desktop_channels WHERE id = ?", id); err != nil {
		return fmt.Errorf("delete desktop channel: %w", err)
	}
	return nil
}

func nullableTime(value *time.Time) any {
	if value == nil {
		return nil
	}
	return value.UTC().Format(time.RFC3339Nano)
}

func formatTime(value time.Time) string {
	return value.UTC().Format(time.RFC3339Nano)
}

type rowScanner interface {
	Scan(dest ...any) error
}

func scanChannel(scanner rowScanner) (*Channel, error) {
	var channel Channel
	var modelsJSON string
	var enabled int
	var lastLatency, lastTTFT, lastPing, injectTokens sql.NullInt64
	var lastChecked, cooldownUntil, injectChecked, createdAt, updatedAt sql.NullString
	if err := scanner.Scan(&channel.ID, &channel.Name, &channel.Provider, &channel.APIMode,
		&channel.Endpoint, &channel.APIKey, &channel.PrimaryModel, &modelsJSON, &enabled,
		&channel.IntervalSeconds, &channel.LastStatus, &lastLatency, &lastTTFT,
		&lastPing, &channel.LastError, &lastChecked, &channel.ProbeStreak, &channel.FailureCount, &channel.CircuitState,
		&cooldownUntil, &channel.InjectionState, &injectTokens, &channel.InjectionContent, &injectChecked,
		&createdAt, &updatedAt); err != nil {
		return nil, fmt.Errorf("scan desktop channel: %w", err)
	}
	channel.Enabled = enabled != 0
	if err := json.Unmarshal([]byte(modelsJSON), &channel.ExtraModels); err != nil {
		return nil, fmt.Errorf("decode desktop channel models: %w", err)
	}
	if lastLatency.Valid {
		value := int(lastLatency.Int64)
		channel.LastLatencyMs = &value
	}
	if lastTTFT.Valid {
		value := int(lastTTFT.Int64)
		channel.LastTTFTMs = &value
	}
	if lastPing.Valid {
		value := int(lastPing.Int64)
		channel.LastPingLatencyMs = &value
	}
	if injectTokens.Valid {
		value := int(injectTokens.Int64)
		channel.InjectionTokens = &value
	}
	channel.InjectionCheckedAt = parseTime(injectChecked)
	channel.LastCheckedAt = parseTime(lastChecked)
	channel.EffectiveIntervalSeconds = effectiveIntervalSeconds(&channel)
	channel.CooldownUntil = parseTime(cooldownUntil)
	if channel.CircuitState == "" {
		channel.CircuitState = "closed"
	}
	if createdAt.Valid {
		channel.CreatedAt, _ = time.Parse(time.RFC3339Nano, createdAt.String)
	}
	if updatedAt.Valid {
		channel.UpdatedAt, _ = time.Parse(time.RFC3339Nano, updatedAt.String)
	}
	return &channel, nil
}

func parseTime(value sql.NullString) *time.Time {
	if !value.Valid || value.String == "" {
		return nil
	}
	parsed, err := time.Parse(time.RFC3339Nano, value.String)
	if err != nil {
		return nil
	}
	return &parsed
}

// RequestLog stores an audit of a single proxied request through HeiGate.
type RequestLog struct {
	ID               int64     `json:"id"`
	Model            string    `json:"model"`
	ChannelName      string    `json:"channel_name"`
	StatusCode       int       `json:"status_code"`
	LatencyMs        int       `json:"latency_ms"`
	TtftMs           int       `json:"ttft_ms"`
	PromptTokens     int       `json:"prompt_tokens"`
	CompletionTokens int       `json:"completion_tokens"`
	TotalTokens      int       `json:"total_tokens"`
	IsStream         bool      `json:"is_stream"`
	ErrorMessage     string    `json:"error_message"`
	FailoverFrom     string    `json:"failover_from"`
	IsFailover       bool      `json:"is_failover"`
	UpstreamModel    string    `json:"upstream_model,omitempty"`
	Endpoint         string    `json:"endpoint,omitempty"`
	ClientIP         string    `json:"client_ip,omitempty"`
	UserAgent        string    `json:"user_agent,omitempty"`
	RequestPath      string    `json:"request_path,omitempty"`
	FinishReason     string    `json:"finish_reason,omitempty"`
	TraceJSON        string    `json:"trace_json,omitempty"`
	CreatedAt        time.Time `json:"created_at"`
}

func (s *Store) SubscribeLogs() (chan LogEvent, func()) {
	if s == nil {
		ch := make(chan LogEvent)
		close(ch)
		return ch, func() {}
	}
	s.logMu.Lock()
	defer s.logMu.Unlock()
	if s.logSubscribers == nil {
		s.logSubscribers = make(map[chan LogEvent]struct{})
	}
	ch := make(chan LogEvent, 128)
	s.logSubscribers[ch] = struct{}{}
	unsubscribe := func() {
		s.logMu.Lock()
		defer s.logMu.Unlock()
		if s.logSubscribers != nil {
			delete(s.logSubscribers, ch)
		}
	}
	return ch, unsubscribe
}

func (s *Store) BroadcastLog(event LogEvent) {
	if s == nil {
		return
	}
	s.logMu.RLock()
	defer s.logMu.RUnlock()
	for ch := range s.logSubscribers {
		select {
		case ch <- event:
		default:
			// Non-blocking write so slow or laggy consumers don't block gateway operations
		}
	}
}

func (s *Store) RecordLog(ctx context.Context, log *RequestLog) error {
	if s == nil || s.db == nil || log == nil {
		return nil
	}
	if log.CreatedAt.IsZero() {
		log.CreatedAt = time.Now().UTC()
	}
	isFailover := 0
	if log.IsFailover {
		isFailover = 1
	}
	isStream := 0
	if log.IsStream {
		isStream = 1
	}
	if log.TotalTokens == 0 && (log.PromptTokens > 0 || log.CompletionTokens > 0) {
		log.TotalTokens = log.PromptTokens + log.CompletionTokens
	}
	query := `INSERT INTO desktop_request_logs (
		model, channel_name, status_code, latency_ms, ttft_ms, prompt_tokens,
		completion_tokens, total_tokens, is_stream, error_message, failover_from,
		is_failover, upstream_model, endpoint, client_ip, user_agent,
		request_path, finish_reason, trace_json, created_at
	) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
	res, err := s.db.ExecContext(ctx, query,
		log.Model, log.ChannelName, log.StatusCode, log.LatencyMs, log.TtftMs, log.PromptTokens,
		log.CompletionTokens, log.TotalTokens, isStream, log.ErrorMessage, log.FailoverFrom,
		isFailover, log.UpstreamModel, log.Endpoint, log.ClientIP, log.UserAgent,
		log.RequestPath, log.FinishReason, log.TraceJSON, formatTime(log.CreatedAt),
	)
	if err != nil {
		return fmt.Errorf("record desktop request log: %w", err)
	}
	log.ID, _ = res.LastInsertId()
	// Broadcast a copy: the caller keeps mutating the same RequestLog while
	// streaming (token counts every ~200ms), and subscribers marshal the event
	// on their own goroutines — sharing the live pointer is a data race.
	created := *log
	s.BroadcastLog(LogEvent{Type: "created", Log: &created})
	return nil
}

func (s *Store) UpdateLog(ctx context.Context, log *RequestLog) error {
	if s == nil || s.db == nil || log == nil || log.ID <= 0 {
		return nil
	}
	isFailover := 0
	if log.IsFailover {
		isFailover = 1
	}
	isStream := 0
	if log.IsStream {
		isStream = 1
	}
	if log.TotalTokens == 0 && (log.PromptTokens > 0 || log.CompletionTokens > 0) {
		log.TotalTokens = log.PromptTokens + log.CompletionTokens
	}
	query := `UPDATE desktop_request_logs
SET channel_name = ?, status_code = ?, latency_ms = ?, ttft_ms = ?, prompt_tokens = ?,
    completion_tokens = ?, total_tokens = ?, is_stream = ?, error_message = ?, failover_from = ?,
    is_failover = ?, upstream_model = ?, endpoint = ?, client_ip = ?, user_agent = ?,
    request_path = ?, finish_reason = ?, trace_json = ?
WHERE id = ?`
	_, err := s.db.ExecContext(ctx, query,
		log.ChannelName, log.StatusCode, log.LatencyMs, log.TtftMs, log.PromptTokens,
		log.CompletionTokens, log.TotalTokens, isStream, log.ErrorMessage, log.FailoverFrom,
		isFailover, log.UpstreamModel, log.Endpoint, log.ClientIP, log.UserAgent,
		log.RequestPath, log.FinishReason, log.TraceJSON, log.ID,
	)
	if err != nil {
		return fmt.Errorf("update desktop request log: %w", err)
	}
	updated := *log
	s.BroadcastLog(LogEvent{Type: "updated", Log: &updated})
	return nil
}

func (s *Store) ListLogs(ctx context.Context, limit int) ([]*RequestLog, error) {
	if s == nil || s.db == nil {
		return nil, fmt.Errorf("desktop channel store is closed")
	}
	if limit <= 0 || limit > 500 {
		limit = 100
	}
	query := `SELECT id, model, channel_name, status_code, latency_ms, ttft_ms,
       prompt_tokens, completion_tokens, total_tokens, is_stream,
       error_message, failover_from, is_failover,
       COALESCE(upstream_model, ''), COALESCE(endpoint, ''),
       COALESCE(client_ip, ''), COALESCE(user_agent, ''),
       COALESCE(request_path, ''), COALESCE(finish_reason, ''),
       COALESCE(trace_json, ''), created_at
FROM desktop_request_logs ORDER BY id DESC LIMIT ?`
	rows, err := s.db.QueryContext(ctx, query, limit)
	if err != nil {
		return nil, fmt.Errorf("list desktop request logs: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var logs []*RequestLog
	for rows.Next() {
		var l RequestLog
		var isFailover int
		var isStream int
		var createdAt sql.NullString
		if err := rows.Scan(
			&l.ID, &l.Model, &l.ChannelName, &l.StatusCode, &l.LatencyMs, &l.TtftMs,
			&l.PromptTokens, &l.CompletionTokens, &l.TotalTokens, &isStream,
			&l.ErrorMessage, &l.FailoverFrom, &isFailover,
			&l.UpstreamModel, &l.Endpoint, &l.ClientIP, &l.UserAgent,
			&l.RequestPath, &l.FinishReason, &l.TraceJSON, &createdAt,
		); err != nil {
			return nil, fmt.Errorf("scan desktop request log: %w", err)
		}
		l.IsFailover = isFailover != 0
		l.IsStream = isStream != 0
		if parsed := parseTime(createdAt); parsed != nil {
			l.CreatedAt = *parsed
		}
		logs = append(logs, &l)
	}
	return logs, rows.Err()
}

func (s *Store) ClearLogs(ctx context.Context) error {
	if s == nil || s.db == nil {
		return fmt.Errorf("desktop channel store is closed")
	}
	_, err := s.db.ExecContext(ctx, `DELETE FROM desktop_request_logs`)
	if err == nil {
		s.BroadcastLog(LogEvent{Type: "cleared"})
	}
	return err
}

func (s *Store) PruneLogsOlderThan(ctx context.Context, days int) (int64, error) {
	if s == nil || s.db == nil {
		return 0, fmt.Errorf("desktop channel store is closed")
	}
	if days <= 0 {
		return 0, nil
	}
	cutoff := time.Now().UTC().AddDate(0, 0, -days)
	cutoffStr := formatTime(cutoff)
	res, err := s.db.ExecContext(ctx, `DELETE FROM desktop_request_logs WHERE created_at < ?`, cutoffStr)
	if err != nil {
		return 0, fmt.Errorf("prune desktop request logs by age: %w", err)
	}
	affected, _ := res.RowsAffected()
	if affected > 0 {
		s.BroadcastLog(LogEvent{Type: "pruned"})
	}
	return affected, nil
}

// PruneLogsLimit trims oldest logs when total count exceeds maxCount.
func (s *Store) PruneLogsLimit(ctx context.Context, maxCount int) (int64, error) {
	if s == nil || s.db == nil {
		return 0, fmt.Errorf("desktop channel store is closed")
	}
	if maxCount <= 0 {
		return 0, nil
	}
	count, err := s.CountLogs(ctx)
	if err != nil {
		return 0, err
	}
	if count <= int64(maxCount) {
		return 0, nil
	}
	res, err := s.db.ExecContext(ctx, `DELETE FROM desktop_request_logs WHERE id NOT IN (
		SELECT id FROM desktop_request_logs ORDER BY id DESC LIMIT ?
	)`, maxCount)
	if err != nil {
		return 0, fmt.Errorf("prune desktop request logs by limit: %w", err)
	}
	affected, _ := res.RowsAffected()
	if affected > 0 {
		s.BroadcastLog(LogEvent{Type: "pruned"})
	}
	return affected, nil
}

// AutoPruneLogs applies the configured retention days and max log count rules.
func (s *Store) AutoPruneLogs(ctx context.Context) (int64, error) {
	if s == nil || s.db == nil {
		return 0, nil
	}
	retentionDaysStr, _ := s.GetSetting(ctx, "log_retention_days", "7")
	retentionDays, _ := strconv.Atoi(retentionDaysStr)
	if retentionDays < 0 {
		retentionDays = 7
	}

	maxCountStr, _ := s.GetSetting(ctx, "log_max_count", "5000")
	maxCount, _ := strconv.Atoi(maxCountStr)
	if maxCount < 0 {
		maxCount = 5000
	}

	var totalDeleted int64
	if retentionDays > 0 {
		del, err := s.PruneLogsOlderThan(ctx, retentionDays)
		if err == nil {
			totalDeleted += del
		}
	}
	if maxCount > 0 {
		del, err := s.PruneLogsLimit(ctx, maxCount)
		if err == nil {
			totalDeleted += del
		}
	}
	return totalDeleted, nil
}

func (s *Store) CountLogs(ctx context.Context) (int64, error) {
	if s == nil || s.db == nil {
		return 0, fmt.Errorf("desktop channel store is closed")
	}
	var count int64
	err := s.db.QueryRowContext(ctx, `SELECT COUNT(1) FROM desktop_request_logs`).Scan(&count)
	return count, err
}

func (s *Store) GetSetting(ctx context.Context, key string, fallback string) (string, error) {
	if s == nil || s.db == nil {
		return fallback, nil
	}
	var value string
	err := s.db.QueryRowContext(ctx, `SELECT value FROM desktop_settings WHERE key = ?`, key).Scan(&value)
	if err == sql.ErrNoRows {
		return fallback, nil
	}
	if err != nil {
		return fallback, err
	}
	return value, nil
}

func (s *Store) SetSetting(ctx context.Context, key string, value string) error {
	if s == nil || s.db == nil {
		return fmt.Errorf("desktop channel store is closed")
	}
	now := formatTime(time.Now().UTC())
	query := `INSERT INTO desktop_settings (key, value, updated_at) VALUES (?, ?, ?)
ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at`
	_, err := s.db.ExecContext(ctx, query, key, value, now)
	return err
}

// AnalyticsSummary holds overall high-level metrics for the selected time window.
type AnalyticsSummary struct {
	TotalRequests    int64   `json:"total_requests"`
	SuccessRequests  int64   `json:"success_requests"`
	FailedRequests   int64   `json:"failed_requests"`
	SuccessRate      float64 `json:"success_rate"`
	TotalTokens      int64   `json:"total_tokens"`
	PromptTokens     int64   `json:"prompt_tokens"`
	CompletionTokens int64   `json:"completion_tokens"`
	AvgLatencyMs     float64 `json:"avg_latency_ms"`
	AvgTtftMs        float64 `json:"avg_ttft_ms"`
}

// DailyAnalyticsTrend holds usage metrics aggregated by day (YYYY-MM-DD).
type DailyAnalyticsTrend struct {
	Date             string  `json:"date"`
	TotalRequests    int64   `json:"total_requests"`
	SuccessRequests  int64   `json:"success_requests"`
	FailedRequests   int64   `json:"failed_requests"`
	TotalTokens      int64   `json:"total_tokens"`
	PromptTokens     int64   `json:"prompt_tokens"`
	CompletionTokens int64   `json:"completion_tokens"`
	AvgLatencyMs     float64 `json:"avg_latency_ms"`
	AvgTtftMs        float64 `json:"avg_ttft_ms"`
}

// ModelAnalyticsStat holds usage & performance metrics aggregated per model.
type ModelAnalyticsStat struct {
	Model            string  `json:"model"`
	TotalRequests    int64   `json:"total_requests"`
	SuccessRequests  int64   `json:"success_requests"`
	FailedRequests   int64   `json:"failed_requests"`
	SuccessRate      float64 `json:"success_rate"`
	TotalTokens      int64   `json:"total_tokens"`
	PromptTokens     int64   `json:"prompt_tokens"`
	CompletionTokens int64   `json:"completion_tokens"`
	AvgLatencyMs     float64 `json:"avg_latency_ms"`
	MinLatencyMs     int64   `json:"min_latency_ms"`
	MaxLatencyMs     int64   `json:"max_latency_ms"`
	AvgTtftMs        float64 `json:"avg_ttft_ms"`
	TopChannel       string  `json:"top_channel"`
}

// ChannelAnalyticsStat holds performance metrics aggregated per upstream channel.
type ChannelAnalyticsStat struct {
	ChannelName     string  `json:"channel_name"`
	TotalRequests   int64   `json:"total_requests"`
	SuccessRequests int64   `json:"success_requests"`
	FailedRequests  int64   `json:"failed_requests"`
	SuccessRate     float64 `json:"success_rate"`
	TotalTokens     int64   `json:"total_tokens"`
	AvgLatencyMs    float64 `json:"avg_latency_ms"`
	AvgTtftMs       float64 `json:"avg_ttft_ms"`
}

// AnalyticsResult is the complete analytics payload.
type AnalyticsResult struct {
	Days     int                    `json:"days"`
	Summary  AnalyticsSummary       `json:"summary"`
	Daily    []DailyAnalyticsTrend  `json:"daily"`
	Models   []ModelAnalyticsStat   `json:"models"`
	Channels []ChannelAnalyticsStat `json:"channels"`
}

func roundAnalytics(val float64) float64 {
	return math.Round(val*10) / 10
}

func (s *Store) GetAnalytics(ctx context.Context, days int) (*AnalyticsResult, error) {
	if s == nil || s.db == nil {
		return nil, fmt.Errorf("desktop channel store is closed")
	}

	result := &AnalyticsResult{
		Days:     days,
		Daily:    make([]DailyAnalyticsTrend, 0),
		Models:   make([]ModelAnalyticsStat, 0),
		Channels: make([]ChannelAnalyticsStat, 0),
	}

	var cutoffStr string
	now := time.Now()
	if days == 1 {
		localMidnight := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
		cutoffStr = formatTime(localMidnight.UTC())
	} else if days > 1 {
		localStart := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location()).AddDate(0, 0, -(days - 1))
		cutoffStr = formatTime(localStart.UTC())
	}

	buildWhere := func(extraConditions ...string) (string, []any) {
		conditions := make([]string, 0, len(extraConditions)+1)
		var queryArgs []any
		if cutoffStr != "" {
			conditions = append(conditions, "created_at >= ?")
			queryArgs = append(queryArgs, cutoffStr)
		}
		for _, cond := range extraConditions {
			if cond != "" {
				conditions = append(conditions, cond)
			}
		}
		if len(conditions) == 0 {
			return "", queryArgs
		}
		return " WHERE " + strings.Join(conditions, " AND "), queryArgs
	}

	// 1. Overall Summary
	summaryWhere, summaryArgs := buildWhere()
	summaryQuery := `
SELECT
    COUNT(1),
    COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(total_tokens), 0),
    COALESCE(SUM(prompt_tokens), 0),
    COALESCE(SUM(completion_tokens), 0),
    COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
    COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
FROM desktop_request_logs` + summaryWhere

	err := s.db.QueryRowContext(ctx, summaryQuery, summaryArgs...).Scan(
		&result.Summary.TotalRequests,
		&result.Summary.SuccessRequests,
		&result.Summary.FailedRequests,
		&result.Summary.TotalTokens,
		&result.Summary.PromptTokens,
		&result.Summary.CompletionTokens,
		&result.Summary.AvgLatencyMs,
		&result.Summary.AvgTtftMs,
	)
	if err != nil && err != sql.ErrNoRows {
		return nil, fmt.Errorf("query analytics summary: %w", err)
	}
	result.Summary.AvgLatencyMs = roundAnalytics(result.Summary.AvgLatencyMs)
	result.Summary.AvgTtftMs = roundAnalytics(result.Summary.AvgTtftMs)
	if result.Summary.TotalRequests > 0 {
		result.Summary.SuccessRate = roundAnalytics(float64(result.Summary.SuccessRequests) / float64(result.Summary.TotalRequests) * 100)
	}

	// 2. Daily Trends
	dailyWhere, dailyArgs := buildWhere()
	dailyQuery := `
SELECT
    date(created_at, 'localtime') AS day,
    COUNT(1),
    COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(total_tokens), 0),
    COALESCE(SUM(prompt_tokens), 0),
    COALESCE(SUM(completion_tokens), 0),
    COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
    COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
FROM desktop_request_logs` + dailyWhere + `
GROUP BY day
ORDER BY day ASC`

	dailyRows, err := s.db.QueryContext(ctx, dailyQuery, dailyArgs...)
	if err != nil {
		return nil, fmt.Errorf("query analytics daily trends: %w", err)
	}
	defer func() { _ = dailyRows.Close() }()

	for dailyRows.Next() {
		var d DailyAnalyticsTrend
		if err := dailyRows.Scan(
			&d.Date,
			&d.TotalRequests,
			&d.SuccessRequests,
			&d.FailedRequests,
			&d.TotalTokens,
			&d.PromptTokens,
			&d.CompletionTokens,
			&d.AvgLatencyMs,
			&d.AvgTtftMs,
		); err != nil {
			return nil, fmt.Errorf("scan daily trend row: %w", err)
		}
		d.AvgLatencyMs = roundAnalytics(d.AvgLatencyMs)
		d.AvgTtftMs = roundAnalytics(d.AvgTtftMs)
		result.Daily = append(result.Daily, d)
	}

	// 3. Model Top Channels Map
	topChanWhere, topChanArgs := buildWhere("channel_name != ''", "channel_name != '路由中...'")
	topChannelQuery := `
SELECT model, channel_name, COUNT(1) as cnt
FROM desktop_request_logs` + topChanWhere + `
GROUP BY model, channel_name
ORDER BY model, cnt DESC`

	topChannels := make(map[string]string)
	channelRows, err := s.db.QueryContext(ctx, topChannelQuery, topChanArgs...)
	if err == nil {
		defer func() { _ = channelRows.Close() }()
		for channelRows.Next() {
			var m, ch string
			var count int64
			if err := channelRows.Scan(&m, &ch, &count); err == nil {
				if _, exists := topChannels[m]; !exists && ch != "" {
					topChannels[m] = ch
				}
			}
		}
	}

	// 4. Per-Model Stats
	modelWhere, modelArgs := buildWhere()
	modelQuery := `
SELECT
    model,
    COUNT(1),
    COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(total_tokens), 0),
    COALESCE(SUM(prompt_tokens), 0),
    COALESCE(SUM(completion_tokens), 0),
    COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
    COALESCE(MIN(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
    COALESCE(MAX(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
    COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
FROM desktop_request_logs` + modelWhere + `
GROUP BY model
ORDER BY COUNT(1) DESC`

	modelRows, err := s.db.QueryContext(ctx, modelQuery, modelArgs...)
	if err != nil {
		return nil, fmt.Errorf("query analytics model stats: %w", err)
	}
	defer func() { _ = modelRows.Close() }()

	for modelRows.Next() {
		var m ModelAnalyticsStat
		if err := modelRows.Scan(
			&m.Model,
			&m.TotalRequests,
			&m.SuccessRequests,
			&m.FailedRequests,
			&m.TotalTokens,
			&m.PromptTokens,
			&m.CompletionTokens,
			&m.AvgLatencyMs,
			&m.MinLatencyMs,
			&m.MaxLatencyMs,
			&m.AvgTtftMs,
		); err != nil {
			return nil, fmt.Errorf("scan model stat row: %w", err)
		}
		m.AvgLatencyMs = roundAnalytics(m.AvgLatencyMs)
		m.AvgTtftMs = roundAnalytics(m.AvgTtftMs)
		if m.TotalRequests > 0 {
			m.SuccessRate = roundAnalytics(float64(m.SuccessRequests) / float64(m.TotalRequests) * 100)
		}
		m.TopChannel = topChannels[m.Model]
		result.Models = append(result.Models, m)
	}

	// 5. Per-Channel Stats
	chanWhere, chanArgs := buildWhere("channel_name != ''", "channel_name != '路由中...'")
	chanStatsQuery := `
SELECT
    channel_name,
    COUNT(1),
    COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(total_tokens), 0),
    COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
    COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
FROM desktop_request_logs` + chanWhere + `
GROUP BY channel_name
ORDER BY COUNT(1) DESC`

	chRows, err := s.db.QueryContext(ctx, chanStatsQuery, chanArgs...)
	if err == nil {
		defer func() { _ = chRows.Close() }()
		for chRows.Next() {
			var ch ChannelAnalyticsStat
			if err := chRows.Scan(
				&ch.ChannelName,
				&ch.TotalRequests,
				&ch.SuccessRequests,
				&ch.FailedRequests,
				&ch.TotalTokens,
				&ch.AvgLatencyMs,
				&ch.AvgTtftMs,
			); err == nil {
				ch.AvgLatencyMs = roundAnalytics(ch.AvgLatencyMs)
				ch.AvgTtftMs = roundAnalytics(ch.AvgTtftMs)
				if ch.TotalRequests > 0 {
					ch.SuccessRate = roundAnalytics(float64(ch.SuccessRequests) / float64(ch.TotalRequests) * 100)
				}
				result.Channels = append(result.Channels, ch)
			}
		}
	}

	return result, nil
}
