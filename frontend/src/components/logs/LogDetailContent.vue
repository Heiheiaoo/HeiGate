<script setup lang="ts">
import type { RequestLog, TraceStep } from '@/services/gatewayApi'

type Diagnostic = {
  badge: 'success' | 'amber' | 'danger' | 'info'
  codeText: string
  title: string
  message: string
}

const props = defineProps<{
  log: RequestLog
  diagnostic: Diagnostic
}>()

const emit = defineEmits<{
  (e: 'copy-log-json', log: RequestLog): void
  (e: 'copy-error-message', message: string): void
}>()

function formatLatencySec(ms: number | null | undefined): string {
  if (ms == null) return '超时'
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

function calculateTokenSpeed(log: RequestLog): string {
  if (log.completion_tokens && log.latency_ms && log.latency_ms > 0) {
    return `${(log.completion_tokens / (log.latency_ms / 1000)).toFixed(1)} t/s`
  }
  return '-'
}

function parseTraceSteps(log: RequestLog): TraceStep[] {
  if (log.trace_json) {
    try {
      const parsed = JSON.parse(log.trace_json)
      if (Array.isArray(parsed) && parsed.length > 0) return parsed as TraceStep[]
    } catch {
      // Fall back
    }
  }

  if (log.is_failover && log.failover_from) {
    return [
      {
        index: 1,
        channel_name: log.failover_from,
        channel_id: 0,
        endpoint: '',
        model: log.model,
        attempt: 1,
        status_code: 502,
        latency_ms: 0,
        error: '前序供应商请求失败/超时，触发故障轮转',
        succeeded: false,
      },
      {
        index: 2,
        channel_name: log.channel_name,
        channel_id: log.channel_id,
        endpoint: log.endpoint || '',
        model: log.upstream_model || log.model,
        attempt: 1,
        status_code: log.status_code,
        latency_ms: log.latency_ms,
        ttft_ms: log.ttft_ms,
        error: log.error_message,
        succeeded: log.status_code >= 200 && log.status_code < 300,
      },
    ]
  }

  return [
    {
      index: 1,
      channel_name: log.channel_name || '默认渠道',
      channel_id: log.channel_id,
      endpoint: log.endpoint || '',
      model: log.upstream_model || log.model,
      attempt: 1,
      status_code: log.status_code,
      latency_ms: log.latency_ms,
      ttft_ms: log.ttft_ms,
      error: log.error_message,
      succeeded: log.status_code >= 200 && log.status_code < 300,
    },
  ]
}
</script>

<template>
  <div class="m26-modal-body m26-diagnostic-body">
    <!-- 1. Abnormal or Failover Alert Banner (仅在异常、流式生成中或故障轮转时展示) -->
    <div
      v-if="props.log.is_failover || props.log.status_code >= 400 || props.log.status_code === 0"
      class="m26-diag-banner"
      :class="props.diagnostic.badge"
    >
      <div class="m26-diag-banner-header">
        <span class="m26-diag-dot" :class="props.diagnostic.badge"></span>
        <span class="font-bold">{{ props.diagnostic.title }}</span>
        <span class="m26-diag-code-pill font-mono" :class="'code-' + props.diagnostic.badge">
          {{ props.diagnostic.codeText }}
        </span>
      </div>
      <div v-if="props.diagnostic.message" class="m26-diag-banner-msg">
        {{ props.diagnostic.message }}
      </div>
    </div>

    <!-- 2. Core Request Overview Card (模型、渠道、端点、状态集成卡片) -->
    <div class="m26-summary-card">
      <div class="m26-summary-row-top">
        <div class="m26-summary-model-box">
          <span class="m26-summary-model-badge font-mono">{{ props.log.model }}</span>
          <span
            v-if="props.log.upstream_model && props.log.upstream_model !== props.log.model"
            class="m26-summary-upstream-text font-mono"
            :title="'实际向上游请求模型: ' + props.log.upstream_model"
          >
            ➔ {{ props.log.upstream_model }}
          </span>
        </div>

        <div class="flex items-center gap-2">
          <span v-if="props.log.is_stream" class="m26-badge-stream">SSE 流式</span>
          <span
            class="m26-trace-code-pill font-mono"
            :class="props.log.status_code >= 200 && props.log.status_code < 300 ? 'status-200' : props.log.status_code === 429 ? 'status-429' : 'status-err'"
          >
            {{ props.log.status_code === 0 ? '传输中' : props.log.status_code >= 200 && props.log.status_code < 300 ? `${props.log.status_code} OK` : `${props.log.status_code} 异常` }}
          </span>
        </div>
      </div>

      <div class="m26-summary-route-meta">
        <div class="m26-route-item">
          <span class="label">供应商:</span>
          <span class="val font-semibold text-slate-800">{{ props.log.channel_name || '默认渠道' }}</span>
          <span v-if="props.log.channel_id" class="m26-trace-id-badge">ID: {{ props.log.channel_id }}</span>
        </div>
        <div class="m26-route-item flex-1 min-w-0" v-if="props.log.endpoint">
          <span class="label">端点:</span>
          <span class="val font-mono text-xs text-slate-600 truncate" :title="props.log.endpoint">{{ props.log.endpoint }}</span>
        </div>
      </div>
    </div>

    <!-- 3. Failover Execution Trace (仅在发生故障轮转或多级重试时展示链路步骤) -->
    <div v-if="props.log.is_failover && parseTraceSteps(props.log).length > 1" class="m26-log-section">
      <div class="m26-section-subtitle-bar">
        <span class="m26-subtitle-tag">故障轮转链路</span>
        <span class="m26-subtitle-title">多级重试与故障切流过程</span>
      </div>
      <div class="m26-trace-timeline">
        <div
          v-for="step in parseTraceSteps(props.log)"
          :key="step.index"
          class="m26-trace-item"
          :class="{ 'trace-item-success': step.succeeded, 'trace-item-fail': !step.succeeded }"
        >
          <div class="m26-trace-node">
            <span class="m26-trace-index">{{ step.index }}</span>
            <div class="m26-trace-line"></div>
          </div>
          <div class="m26-trace-card">
            <div class="m26-trace-header">
              <div class="flex items-center gap-2">
                <span class="m26-trace-channel-name">{{ step.channel_name || '未知渠道' }}</span>
                <span v-if="step.channel_id" class="m26-trace-id-badge">ID: {{ step.channel_id }}</span>
                <span v-if="step.attempt > 1" class="m26-trace-retry-badge">重试 #{{ step.attempt }}</span>
              </div>
              <div class="flex items-center gap-1.5">
                <span
                  class="m26-trace-code-pill font-mono"
                  :class="step.status_code >= 200 && step.status_code < 300 ? 'status-200' : step.status_code === 429 ? 'status-429' : 'status-err'"
                >
                  {{ step.status_code ? `HTTP ${step.status_code}` : '超时/中断' }}
                </span>
                <span v-if="step.latency_ms > 0" class="m26-trace-latency">{{ formatLatencySec(step.latency_ms) }}</span>
              </div>
            </div>
            <div v-if="step.error || step.raw_error" class="m26-trace-error-box">
              <span class="error-icon">⚠️</span>
              <span class="font-mono text-xs">{{ step.error || step.raw_error }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 4. Performance & Token Bento Grid (耗时与用量紧凑指标卡) -->
    <div class="m26-log-section">
      <div class="m26-section-subtitle-bar">
        <span class="m26-subtitle-tag">吞吐性能</span>
        <span class="m26-subtitle-title">Token 用量与响应耗时</span>
      </div>
      <div class="m26-perf-grid">
        <div class="m26-perf-card">
          <span class="label">全程总耗时</span>
          <span class="val font-mono">{{ formatLatencySec(props.log.latency_ms) }}</span>
        </div>
        <div class="m26-perf-card">
          <span class="label">首字耗时 (TTFT)</span>
          <span class="val font-mono">{{ formatLatencySec(props.log.ttft_ms || props.log.latency_ms) }}</span>
        </div>
        <div class="m26-perf-card">
          <span class="label">生成速率</span>
          <span class="val font-mono">{{ calculateTokenSpeed(props.log) }}</span>
        </div>
        <div class="m26-perf-card">
          <span class="label">提示词 Token</span>
          <span class="val font-mono">{{ props.log.prompt_tokens ?? 0 }}</span>
        </div>
        <div class="m26-perf-card">
          <span class="label">补全 Token</span>
          <span class="val font-mono text-emerald-600 font-semibold">{{ props.log.completion_tokens ?? 0 }}</span>
        </div>
        <div class="m26-perf-card">
          <span class="label">总 Token</span>
          <span class="val font-mono font-bold">{{ props.log.total_tokens ?? ((props.log.prompt_tokens || 0) + (props.log.completion_tokens || 0)) }}</span>
        </div>
      </div>
    </div>

    <!-- 5. Upstream Error Message (仅在有报错时展示) -->
    <div v-if="props.log.error_message" class="m26-diag-raw-section">
      <div class="m26-diag-raw-header">
        <span class="label text-rose-600 font-semibold text-xs">⚠️ 上游报错回包 (Error Diagnostics)</span>
        <button class="m26-text-btn" @click="emit('copy-error-message', props.log.error_message)">复制报错</button>
      </div>
      <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ props.log.error_message }}</code></pre>
    </div>

    <!-- 6. Collapsible Raw JSON (默认收起，一键展开或复制，不占用垂直视线) -->
    <details class="m26-raw-json-details">
      <summary class="m26-raw-json-summary">
        <span class="font-medium text-xs text-slate-500 hover:text-slate-800">▶ 展开原始流水数据 (JSON)</span>
        <button class="m26-text-btn" @click.stop="emit('copy-log-json', props.log)">复制完整 JSON</button>
      </summary>
      <pre class="m26-diag-raw-code font-mono text-xs mt-2"><code>{{ JSON.stringify(props.log, null, 2) }}</code></pre>
    </details>
  </div>
</template>

<style scoped>
.m26-diagnostic-body {
  padding: 18px 22px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  max-height: calc(85vh - 120px);
  overflow-y: auto;
}

/* Diagnostic Alert Banner */
.m26-diag-banner {
  padding: 10px 14px;
  border-radius: 10px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.m26-diag-banner.amber {
  background: rgba(245, 158, 11, 0.08);
  border: 1px solid rgba(245, 158, 11, 0.28);
}
.m26-diag-banner.danger {
  background: rgba(225, 29, 72, 0.06);
  border: 1px solid rgba(225, 29, 72, 0.24);
}
.m26-diag-banner.info {
  background: rgba(59, 130, 246, 0.06);
  border: 1px solid rgba(59, 130, 246, 0.25);
}
.m26-diag-banner.success {
  background: rgba(16, 185, 129, 0.06);
  border: 1px solid rgba(16, 185, 129, 0.25);
}
.m26-diag-banner-header {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12px;
  font-weight: 600;
  color: #1e293b;
}
.m26-diag-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}
.m26-diag-dot.amber { background: var(--m26-amber); box-shadow: 0 0 6px rgba(217, 119, 6, 0.5); }
.m26-diag-dot.danger { background: var(--m26-rose); box-shadow: 0 0 6px rgba(225, 29, 72, 0.5); }
.m26-diag-dot.info { background: #2563eb; box-shadow: 0 0 6px rgba(59, 130, 246, 0.5); }
.m26-diag-dot.success { background: var(--m26-emerald); box-shadow: 0 0 6px rgba(5, 150, 105, 0.5); }

.m26-diag-code-pill {
  margin-left: auto;
  font-size: 11px;
  padding: 1px 7px;
  border-radius: 4px;
  font-weight: 700;
}
.m26-diag-code-pill.code-amber { background: rgba(245, 158, 11, 0.14); color: #b45309; }
.m26-diag-code-pill.code-danger { background: rgba(225, 29, 72, 0.12); color: var(--m26-rose); }
.m26-diag-code-pill.code-info { background: rgba(59, 130, 246, 0.14); color: #2563eb; }
.m26-diag-code-pill.code-success { background: rgba(5, 150, 105, 0.12); color: var(--m26-emerald); }

.m26-diag-banner-msg {
  font-size: 12px;
  color: #475569;
  line-height: 1.45;
  padding-left: 14px;
}

/* Core Summary Card */
.m26-summary-card {
  padding: 12px 14px;
  background: #f8fafc;
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.m26-summary-row-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}
.m26-summary-model-box {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
}
.m26-summary-model-badge {
  font-size: 13.5px;
  font-weight: 700;
  color: #0f172a;
  letter-spacing: -0.01em;
}
.m26-summary-upstream-text {
  font-size: 11px;
  color: #64748b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.m26-badge-stream {
  font-size: 10.5px;
  font-weight: 600;
  color: #2563eb;
  background: #dbeafe;
  padding: 1.5px 6px;
  border-radius: 4px;
}
.m26-summary-route-meta {
  display: flex;
  align-items: center;
  gap: 16px;
  font-size: 11.5px;
  color: #64748b;
  flex-wrap: wrap;
  border-top: 1px solid rgba(0, 0, 0, 0.04);
  padding-top: 7px;
}
.m26-route-item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.m26-route-item .label {
  color: #94a3b8;
  font-size: 11px;
}
.m26-route-item .val {
  color: #334155;
}

/* Sections & Perf Grid */
.m26-log-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.m26-section-subtitle-bar {
  display: flex;
  align-items: center;
  gap: 6px;
}
.m26-subtitle-tag {
  font-size: 10px;
  font-weight: 700;
  padding: 1.5px 5px;
  background: #e2e8f0;
  color: #475569;
  border-radius: 4px;
}
.m26-subtitle-title {
  font-size: 11.5px;
  font-weight: 600;
  color: #475569;
}
.m26-perf-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
}
.m26-perf-card {
  padding: 8px 12px;
  background: #f8fafc;
  border: 1px solid var(--m26-border);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.m26-perf-card .label {
  font-size: 10.5px;
  color: var(--m26-text-muted);
}
.m26-perf-card .val {
  font-size: 13.5px;
  font-weight: 700;
  color: var(--m26-text);
}

/* Failover Trace */
.m26-trace-timeline {
  display: flex;
  flex-direction: column;
  padding: 2px 0;
}
.m26-trace-item {
  display: flex;
  gap: 10px;
  position: relative;
}
.m26-trace-node {
  display: flex;
  flex-direction: column;
  align-items: center;
  width: 20px;
  flex-shrink: 0;
}
.m26-trace-index {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 700;
  background: #e2e8f0;
  color: #475569;
  border: 2px solid #fff;
  z-index: 1;
}
.trace-item-success .m26-trace-index { background: #10b981; color: #fff; }
.trace-item-fail .m26-trace-index { background: #f43f5e; color: #fff; }
.m26-trace-line {
  flex: 1;
  width: 2px;
  background: #e2e8f0;
  min-height: 18px;
}
.m26-trace-item:last-child .m26-trace-line { display: none; }
.m26-trace-card {
  flex: 1;
  padding: 8px 10px;
  margin-bottom: 8px;
  background: #fff;
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 8px;
}
.trace-item-success .m26-trace-card { border-left: 3px solid #10b981; }
.trace-item-fail .m26-trace-card { border-left: 3px solid #f43f5e; }
.m26-trace-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
.m26-trace-channel-name { font-size: 12px; font-weight: 700; color: #1e293b; }
.m26-trace-id-badge { font-size: 10px; padding: 1px 5px; border-radius: 4px; color: #64748b; background: #f1f5f9; }
.m26-trace-retry-badge { font-size: 10px; padding: 1px 5px; border-radius: 4px; color: #d97706; background: #fef3c7; font-weight: 600; }
.m26-trace-code-pill { font-size: 10.5px; font-weight: 700; padding: 1px 6px; border-radius: 4px; }
.m26-trace-code-pill.status-200 { color: #059669; background: #ecfdf5; }
.m26-trace-code-pill.status-429 { color: #b45309; background: #fffbeb; }
.m26-trace-code-pill.status-err { color: #e11d48; background: #fff1f2; }
.m26-trace-latency { font-size: 11px; font-family: ui-monospace, monospace; color: #64748b; }
.m26-trace-error-box {
  margin-top: 4px;
  padding: 4px 6px;
  background: #fff1f2;
  border: 1px solid #ffe4e6;
  border-radius: 6px;
  display: flex;
  align-items: flex-start;
  gap: 4px;
  color: #e11d48;
  word-break: break-all;
}

/* Error Raw Section */
.m26-diag-raw-section { display: flex; flex-direction: column; gap: 4px; }
.m26-diag-raw-header { display: flex; justify-content: space-between; align-items: center; }
.m26-diag-raw-code {
  padding: 10px 12px;
  background: #0f172a;
  color: #f1f5f9;
  border-radius: 8px;
  max-height: 140px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.4;
}

/* Collapsible JSON */
.m26-raw-json-details {
  border-top: 1px solid rgba(0, 0, 0, 0.06);
  padding-top: 8px;
}
.m26-raw-json-summary {
  display: flex;
  align-items: center;
  justify-content: space-between;
  cursor: pointer;
  user-select: none;
}
.m26-text-btn {
  background: transparent;
  border: none;
  font-size: 11px;
  font-weight: 550;
  color: #4f46e5;
  cursor: pointer;
  padding: 1px 4px;
  border-radius: 4px;
}
.m26-text-btn:hover {
  text-decoration: underline;
  background: rgba(79, 70, 229, 0.06);
}

@media (max-width: 640px) {
  .m26-perf-grid { grid-template-columns: repeat(2, 1fr); }
}
</style>
