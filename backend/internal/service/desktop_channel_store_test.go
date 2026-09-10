package service

import (
	"context"
	"database/sql"
	"testing"
	"time"

	_ "modernc.org/sqlite"
)

func TestDesktopChannelStoreCRUD(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_channel_store_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}

	channel := &DesktopChannel{
		Name: "Test", Provider: MonitorProviderOpenAI, APIMode: MonitorAPIModeResponses,
		Endpoint: "https://example.com", APIKey: "sk-test", PrimaryModel: "gpt-test",
		ExtraModels: []string{"gpt-extra"}, Enabled: true, IntervalSeconds: 60, Priority: 2,
	}
	if err := store.Save(context.Background(), channel); err != nil {
		t.Fatal(err)
	}
	if channel.ID == 0 {
		t.Fatal("expected inserted channel id")
	}

	loaded, err := store.Get(context.Background(), channel.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.APIKey != "sk-test" || loaded.APIMode != MonitorAPIModeResponses || len(loaded.ExtraModels) != 1 {
		t.Fatalf("loaded channel mismatch: %#v", loaded)
	}

	loaded.Enabled = false
	loaded.LastStatus = MonitorStatusFailed
	if err := store.Save(context.Background(), loaded); err != nil {
		t.Fatal(err)
	}
	updated, err := store.Get(context.Background(), channel.ID)
	if err != nil {
		t.Fatal(err)
	}
	if updated.Enabled || updated.LastStatus != MonitorStatusFailed {
		t.Fatalf("update was not persisted: %#v", updated)
	}

	if err := store.Delete(context.Background(), channel.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Get(context.Background(), channel.ID); err == nil {
		t.Fatal("expected deleted channel lookup to fail")
	}
}

func TestDesktopChannelStoreLogs(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_channel_logs_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}

	reqLog := &DesktopRequestLog{
		Model:            "glm-5.3-flash",
		ChannelName:      "B.AI",
		StatusCode:       200,
		LatencyMs:        350,
		TtftMs:           220,
		PromptTokens:     1024,
		CompletionTokens: 50,
		TotalTokens:      1074,
		IsStream:         true,
		UpstreamModel:    "glm-5.3-flash-free",
		Endpoint:         "https://api.b.ai",
		ClientIP:         "127.0.0.1",
		UserAgent:        "Cline/3.0",
		RequestPath:      "/v1/chat/completions",
		FinishReason:     "stop",
		TraceJSON:        `[{"step":1}]`,
	}
	if err := store.RecordLog(context.Background(), reqLog); err != nil {
		t.Fatalf("failed to record log: %v", err)
	}
	if reqLog.ID == 0 {
		t.Fatal("expected inserted log id")
	}

	logs, err := store.ListLogs(context.Background(), 10)
	if err != nil {
		t.Fatalf("failed to list logs: %v", err)
	}
	if len(logs) != 1 || logs[0].Model != "glm-5.3-flash" || logs[0].LatencyMs != 350 || logs[0].TtftMs != 220 || logs[0].PromptTokens != 1024 || logs[0].CompletionTokens != 50 || !logs[0].IsStream || logs[0].UpstreamModel != "glm-5.3-flash-free" || logs[0].Endpoint != "https://api.b.ai" || logs[0].ClientIP != "127.0.0.1" || logs[0].UserAgent != "Cline/3.0" || logs[0].RequestPath != "/v1/chat/completions" || logs[0].FinishReason != "stop" || logs[0].TraceJSON != `[{"step":1}]` {
		t.Fatalf("unexpected logs: %#v", logs)
	}

	// Test UpdateLog (stream completion phase)
	reqLog.LatencyMs = 2800
	reqLog.CompletionTokens = 360
	reqLog.TotalTokens = 1384
	reqLog.ErrorMessage = "stream interrupted"
	reqLog.StatusCode = 502
	if err := store.UpdateLog(context.Background(), reqLog); err != nil {
		t.Fatalf("failed to update log: %v", err)
	}

	logsAfterUpdate, err := store.ListLogs(context.Background(), 10)
	if err != nil {
		t.Fatalf("failed to list logs after update: %v", err)
	}
	if len(logsAfterUpdate) != 1 || logsAfterUpdate[0].LatencyMs != 2800 || logsAfterUpdate[0].StatusCode != 502 || logsAfterUpdate[0].CompletionTokens != 360 {
		t.Fatalf("unexpected updated log: %#v", logsAfterUpdate[0])
	}

	// Test ClearLogs
	if err := store.ClearLogs(context.Background()); err != nil {
		t.Fatalf("failed to clear logs: %v", err)
	}
	logsAfterClear, err := store.ListLogs(context.Background(), 10)
	if err != nil {
		t.Fatalf("failed to list logs after clear: %v", err)
	}
	if len(logsAfterClear) != 0 {
		t.Fatalf("expected 0 logs after clear, got %d", len(logsAfterClear))
	}
}

func TestDesktopChannelStoreSubscribeLogs(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_channel_sub_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}

	ch, unsubscribe := store.SubscribeLogs()
	defer unsubscribe()

	testLog := &DesktopRequestLog{
		Model:       "test-model",
		ChannelName: "TestChannel",
		StatusCode:  0,
	}
	if err := store.RecordLog(context.Background(), testLog); err != nil {
		t.Fatal(err)
	}

	select {
	case evt := <-ch:
		if evt.Type != "created" || evt.Log == nil || evt.Log.Model != "test-model" {
			t.Fatalf("unexpected created event: %#v", evt)
		}
	case <-time.After(1 * time.Second):
		t.Fatal("timed out waiting for created event")
	}

	testLog.StatusCode = 200
	testLog.LatencyMs = 120
	if err := store.UpdateLog(context.Background(), testLog); err != nil {
		t.Fatal(err)
	}

	select {
	case evt := <-ch:
		if evt.Type != "updated" || evt.Log == nil || evt.Log.StatusCode != 200 {
			t.Fatalf("unexpected updated event: %#v", evt)
		}
	case <-time.After(1 * time.Second):
		t.Fatal("timed out waiting for updated event")
	}

	if err := store.ClearLogs(context.Background()); err != nil {
		t.Fatal(err)
	}

	select {
	case evt := <-ch:
		if evt.Type != "cleared" {
			t.Fatalf("unexpected cleared event: %#v", evt)
		}
	case <-time.After(1 * time.Second):
		t.Fatal("timed out waiting for cleared event")
	}
}

func TestDesktopChannelStorePruneAndAutoPrune(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_channel_prune_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}

	// Insert 10 logs
	for i := 1; i <= 10; i++ {
		l := &DesktopRequestLog{
			Model:       "test-model",
			ChannelName: "TestChannel",
			StatusCode:  200,
			CreatedAt:   time.Now().UTC(),
		}
		if err := store.RecordLog(context.Background(), l); err != nil {
			t.Fatal(err)
		}
	}

	count, err := store.CountLogs(context.Background())
	if err != nil || count != 10 {
		t.Fatalf("expected 10 logs, got %d, err: %v", count, err)
	}

	// Prune limit: keep 6
	del, err := store.PruneLogsLimit(context.Background(), 6)
	if err != nil {
		t.Fatalf("failed to prune logs by limit: %v", err)
	}
	if del != 4 {
		t.Fatalf("expected 4 logs deleted, got %d", del)
	}
	count, _ = store.CountLogs(context.Background())
	if count != 6 {
		t.Fatalf("expected 6 logs remaining, got %d", count)
	}

	// Test AutoPruneLogs with settings
	if err := store.SetSetting(context.Background(), "log_max_count", "3"); err != nil {
		t.Fatal(err)
	}
	if err := store.SetSetting(context.Background(), "log_retention_days", "7"); err != nil {
		t.Fatal(err)
	}

	delAuto, err := store.AutoPruneLogs(context.Background())
	if err != nil {
		t.Fatalf("failed to auto prune: %v", err)
	}
	if delAuto != 3 {
		t.Fatalf("expected 3 deleted by auto prune limit, got %d", delAuto)
	}
	count, _ = store.CountLogs(context.Background())
	if count != 3 {
		t.Fatalf("expected 3 logs remaining, got %d", count)
	}
}

func TestDesktopChannelStoreGetAnalytics(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_channel_analytics_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}

	ctx := context.Background()
	now := time.Now().UTC()

	// Insert test logs
	logs := []*DesktopRequestLog{
		{
			Model:            "gpt-4o",
			ChannelName:      "OpenAI-Main",
			StatusCode:       200,
			LatencyMs:        450,
			TtftMs:           150,
			PromptTokens:     100,
			CompletionTokens: 50,
			TotalTokens:      150,
			CreatedAt:        now,
		},
		{
			Model:            "gpt-4o",
			ChannelName:      "OpenAI-Main",
			StatusCode:       200,
			LatencyMs:        550,
			TtftMs:           170,
			PromptTokens:     200,
			CompletionTokens: 100,
			TotalTokens:      300,
			CreatedAt:        now,
		},
		{
			Model:            "claude-3-7-sonnet",
			ChannelName:      "Anthropic-Direct",
			StatusCode:       500,
			LatencyMs:        1200,
			TtftMs:           0,
			PromptTokens:     50,
			CompletionTokens: 0,
			TotalTokens:      50,
			CreatedAt:        now,
		},
	}

	for _, l := range logs {
		if err := store.RecordLog(ctx, l); err != nil {
			t.Fatalf("failed to record log: %v", err)
		}
	}

	// Test 7-day analytics
	res, err := store.GetAnalytics(ctx, 7)
	if err != nil {
		t.Fatalf("GetAnalytics failed: %v", err)
	}

	if res.Summary.TotalRequests != 3 {
		t.Fatalf("expected 3 total requests, got %d", res.Summary.TotalRequests)
	}
	if res.Summary.SuccessRequests != 2 {
		t.Fatalf("expected 2 successful requests, got %d", res.Summary.SuccessRequests)
	}
	if res.Summary.FailedRequests != 1 {
		t.Fatalf("expected 1 failed request, got %d", res.Summary.FailedRequests)
	}
	if res.Summary.TotalTokens != 500 {
		t.Fatalf("expected 500 total tokens, got %d", res.Summary.TotalTokens)
	}
	if len(res.Models) != 2 {
		t.Fatalf("expected 2 models, got %d", len(res.Models))
	}
	if res.Models[0].Model != "gpt-4o" {
		t.Fatalf("expected top model gpt-4o, got %s", res.Models[0].Model)
	}
	if res.Models[0].TopChannel != "OpenAI-Main" {
		t.Fatalf("expected top channel OpenAI-Main, got %s", res.Models[0].TopChannel)
	}
	if len(res.Daily) == 0 {
		t.Fatalf("expected at least 1 daily trend entry, got 0")
	}
}
