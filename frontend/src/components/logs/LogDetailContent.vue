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

function parseTraceSteps(log: RequestLog): TraceStep[] {
  if (log.trace_json) {
    try {
      const parsed = JSON.parse(log.trace_json)
      if (Array.isArray(parsed) && parsed.length > 0) return parsed as TraceStep[]
    } catch {
      // Fall back to the synthesized trace for malformed historical data.
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
        error: '前序供应商请求失败/超时/熔断，触发故障轮转',
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

function calculateTokenSpeed(log: RequestLog): string {
  if (log.completion_tokens && log.latency_ms && log.latency_ms > 0) {
    return `${(log.completion_tokens / (log.latency_ms / 1000)).toFixed(1)} t/s`
  }
  return '-'
}
</script>

<template>
  <div class="m26-modal-body m26-diagnostic-body">
    <div class="m26-diag-card" :class="props.diagnostic.badge">
      <div class="m26-diag-card-title">
        <span class="m26-diag-dot" :class="props.diagnostic.badge"></span>
        <span>{{ props.diagnostic.title }}</span>
        <span class="m26-diag-code-pill" :class="'code-' + props.diagnostic.badge">
          {{ props.diagnostic.codeText }}
        </span>
      </div>
      <div class="m26-diag-main-msg">{{ props.diagnostic.message }}</div>
    </div>

    <div class="m26-log-section">
      <div class="m26-section-subtitle-bar">
        <span class="m26-subtitle-tag">链路追踪</span>
        <span class="m26-subtitle-title">多级重试与故障轮转链路 (Execution Trace)</span>
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
                  class="m26-trace-code-pill"
                  :class="step.status_code >= 200 && step.status_code < 300 ? 'status-200' : step.status_code === 429 ? 'status-429' : 'status-err'"
                >
                  {{ step.status_code ? `HTTP ${step.status_code}` : '超时/中断' }}
                </span>
                <span v-if="step.latency_ms > 0" class="m26-trace-latency">{{ formatLatencySec(step.latency_ms) }}</span>
              </div>
            </div>
            <div class="m26-trace-body">
              <div class="m26-trace-spec-row">
                <span class="label">端点:</span>
                <span class="val font-mono truncate" :title="step.endpoint">{{ step.endpoint || '默认网关路由' }}</span>
              </div>
              <div class="m26-trace-spec-row">
                <span class="label">模型:</span>
                <span class="val font-mono">{{ step.model }}</span>
                <span v-if="step.ttft_ms" class="val-ttft">TTFT: {{ formatLatencySec(step.ttft_ms) }}</span>
              </div>
              <div v-if="step.error || step.raw_error" class="m26-trace-error-box">
                <span class="error-icon">⚠️</span>
                <span class="font-mono text-xs">{{ step.error || step.raw_error }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div class="m26-log-section">
      <div class="m26-section-subtitle-bar">
        <span class="m26-subtitle-tag">吞吐性能</span>
        <span class="m26-subtitle-title">Token 用量与响应延迟</span>
      </div>
      <div class="m26-perf-grid">
        <div class="m26-perf-card"><span class="label">首字耗时 (TTFT)</span><span class="val font-mono">{{ formatLatencySec(props.log.ttft_ms || props.log.latency_ms) }}</span></div>
        <div class="m26-perf-card"><span class="label">全程总耗时</span><span class="val font-mono">{{ formatLatencySec(props.log.latency_ms) }}</span></div>
        <div class="m26-perf-card"><span class="label">生成速率</span><span class="val font-mono">{{ calculateTokenSpeed(props.log) }}</span></div>
        <div class="m26-perf-card"><span class="label">提示词 Token</span><span class="val font-mono">{{ props.log.prompt_tokens ?? 0 }}</span></div>
        <div class="m26-perf-card"><span class="label">补全 Token</span><span class="val font-mono text-emerald-600">{{ props.log.completion_tokens ?? 0 }}</span></div>
        <div class="m26-perf-card"><span class="label">总 Token</span><span class="val font-mono">{{ props.log.total_tokens ?? ((props.log.prompt_tokens || 0) + (props.log.completion_tokens || 0)) }}</span></div>
      </div>
    </div>

    <div class="m26-log-section">
      <div class="m26-section-subtitle-bar">
        <span class="m26-subtitle-tag">调用环境</span>
        <span class="m26-subtitle-title">客户端与网关路由参数</span>
      </div>
      <div class="m26-diag-meta-grid">
        <div class="m26-diag-meta-item"><span class="label">客户端地址 (Client IP)</span><span class="value font-mono">{{ props.log.client_ip || '127.0.0.1 (本地)' }}</span></div>
        <div class="m26-diag-meta-item"><span class="label">请求接口 (Endpoint Path)</span><span class="value font-mono">{{ props.log.request_path || '/v1/chat/completions' }}</span></div>
        <div class="m26-diag-meta-item"><span class="label">请求模型 (Requested)</span><span class="value font-mono">{{ props.log.model || '通用模型' }}</span></div>
        <div class="m26-diag-meta-item"><span class="label">上游映射模型 (Upstream)</span><span class="value font-mono">{{ props.log.upstream_model || props.log.model || '通用模型' }}</span></div>
        <div class="m26-diag-meta-item"><span class="label">传输类型</span><span class="value">{{ props.log.is_stream ? 'SSE 流式传输 (Server-Sent Events)' : '同步阻塞传输 (Blocking)' }}</span></div>
        <div class="m26-diag-meta-item"><span class="label">结束状态 (Finish Reason)</span><span class="value font-mono">{{ props.log.finish_reason || (props.log.status_code === 200 ? 'stop' : 'unknown') }}</span></div>
        <div class="m26-diag-meta-item col-span-2"><span class="label">最终承载渠道</span><span class="value">{{ props.log.channel_name || '默认' }} (ID: {{ props.log.channel_id }}) · {{ props.log.endpoint || '默认地址' }}</span></div>
        <div v-if="props.log.user_agent" class="m26-diag-meta-item col-span-2"><span class="label">客户端软件 (User-Agent)</span><span class="value font-mono text-xs" :title="props.log.user_agent">{{ props.log.user_agent }}</span></div>
      </div>
    </div>

    <div v-if="props.log.error_message" class="m26-diag-raw-section">
      <div class="m26-diag-raw-header">
        <span class="label">上游异常报错回包 (Error Diagnostics)</span>
        <button class="m26-text-btn" @click="emit('copy-error-message', props.log.error_message)">复制报错</button>
      </div>
      <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ props.log.error_message }}</code></pre>
    </div>

    <div class="m26-diag-raw-section">
      <div class="m26-diag-raw-header">
        <span class="label">完整流水数据 (JSON)</span>
        <button class="m26-text-btn" @click="emit('copy-log-json', props.log)">复制完整 JSON</button>
      </div>
      <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ JSON.stringify(props.log, null, 2) }}</code></pre>
    </div>
  </div>
</template>

<style scoped>
.m26-diagnostic-body { padding: 20px 24px; display: flex; flex-direction: column; gap: 16px; max-height: calc(88vh - 130px); overflow-y: auto; }
.m26-diag-card { padding: 14px 16px; border-radius: 12px; display: flex; flex-direction: column; gap: 8px; }
.m26-diag-card.success { background: rgba(16, 185, 129, 0.05); border: 1px solid rgba(16, 185, 129, 0.25); }
.m26-diag-card.amber { background: rgba(245, 158, 11, 0.06); border: 1px solid rgba(245, 158, 11, 0.28); }
.m26-diag-card.info { background: rgba(59, 130, 246, 0.06); border: 1px solid rgba(59, 130, 246, 0.25); }
.m26-diag-card.danger { background: rgba(225, 29, 72, 0.05); border: 1px solid rgba(225, 29, 72, 0.2); }
.m26-diag-card-title { display: flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 700; color: var(--m26-text); }
.m26-diag-dot { width: 7px; height: 7px; border-radius: 50%; }
.m26-diag-dot.success { background: var(--m26-emerald); box-shadow: 0 0 6px rgba(5, 150, 105, 0.5); }
.m26-diag-dot.amber { background: var(--m26-amber); box-shadow: 0 0 6px rgba(217, 119, 6, 0.5); }
.m26-diag-dot.info { background: #2563eb; box-shadow: 0 0 6px rgba(59, 130, 246, 0.5); }
.m26-diag-dot.danger { background: var(--m26-rose); box-shadow: 0 0 6px rgba(225, 29, 72, 0.5); }
.m26-diag-code-pill { margin-left: auto; font-family: ui-monospace, monospace; font-size: 11px; padding: 1px 7px; border-radius: 4px; font-weight: 700; }
.m26-diag-code-pill.code-success { background: rgba(5, 150, 105, 0.12); color: var(--m26-emerald); }
.m26-diag-code-pill.code-amber { background: rgba(245, 158, 11, 0.14); color: #b45309; }
.m26-diag-code-pill.code-info { background: rgba(59, 130, 246, 0.14); color: #2563eb; }
.m26-diag-code-pill.code-danger { background: rgba(225, 29, 72, 0.12); color: var(--m26-rose); }
.m26-diag-main-msg { font-size: 13px; font-weight: 600; color: #1e293b; line-height: 1.5; }
.m26-log-section { display: flex; flex-direction: column; gap: 8px; margin-top: 4px; }
.m26-section-subtitle-bar { display: flex; align-items: center; gap: 8px; margin-bottom: 2px; }
.m26-subtitle-tag { font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; padding: 2px 6px; background: #e2e8f0; color: #475569; border-radius: 4px; }
.m26-subtitle-title { font-size: 12px; font-weight: 600; color: #334155; }
.m26-diag-meta-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px; }
.m26-diag-meta-item { padding: 10px 12px; background: #f8fafc; border: 1px solid var(--m26-border); border-radius: 8px; display: flex; flex-direction: column; gap: 4px; }
.m26-diag-meta-item .label, .m26-perf-card .label, .m26-diag-raw-header .label { font-size: 11px; color: var(--m26-text-muted); }
.m26-diag-meta-item .value { font-size: 12px; font-weight: 600; color: var(--m26-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.m26-diag-raw-section { display: flex; flex-direction: column; gap: 6px; }
.m26-diag-raw-header { display: flex; justify-content: space-between; align-items: center; }
.m26-diag-raw-header .label { font-weight: 600; }
.m26-diag-raw-code { padding: 10px 14px; background: #0f172a; color: #f1f5f9; border-radius: 8px; max-height: 160px; overflow-y: auto; white-space: pre-wrap; word-break: break-all; line-height: 1.4; }
.m26-trace-timeline { display: flex; flex-direction: column; padding: 4px 0; }
.m26-trace-item { display: flex; gap: 12px; position: relative; }
.m26-trace-node { display: flex; flex-direction: column; align-items: center; width: 24px; flex-shrink: 0; }
.m26-trace-index { width: 22px; height: 22px; border-radius: 50%; display: flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 700; background: #e2e8f0; color: #475569; border: 2px solid #fff; box-shadow: 0 1px 3px rgba(0,0,0,0.1); z-index: 1; }
.trace-item-success .m26-trace-index { background: #10b981; color: #fff; }
.trace-item-fail .m26-trace-index { background: #f43f5e; color: #fff; }
.m26-trace-line { flex: 1; width: 2px; background: #e2e8f0; min-height: 24px; }
.m26-trace-item:last-child .m26-trace-line { display: none; }
.m26-trace-card { flex: 1; padding: 10px 12px; margin-bottom: 10px; background: #fff; border: 1px solid rgba(0,0,0,0.08); border-radius: 10px; box-shadow: 0 1px 3px rgba(0,0,0,0.03); }
.trace-item-success .m26-trace-card { border-left: 3px solid #10b981; }
.trace-item-fail .m26-trace-card { border-left: 3px solid #f43f5e; }
.m26-trace-header { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 6px; }
.m26-trace-channel-name { font-size: 13px; font-weight: 700; color: #1e293b; }
.m26-trace-id-badge, .m26-trace-retry-badge { font-size: 10px; padding: 1px 5px; border-radius: 4px; }
.m26-trace-id-badge { color: #64748b; background: #f1f5f9; }
.m26-trace-retry-badge { color: #d97706; background: #fef3c7; font-weight: 600; }
.m26-trace-code-pill { font-size: 11px; font-weight: 700; padding: 1px 6px; border-radius: 4px; }
.m26-trace-code-pill.status-200 { color: #059669; background: #ecfdf5; }
.m26-trace-code-pill.status-429 { color: #b45309; background: #fffbeb; }
.m26-trace-code-pill.status-err { color: #e11d48; background: #fff1f2; }
.m26-trace-latency { font-size: 11px; font-family: ui-monospace, monospace; color: #64748b; }
.m26-trace-body { display: flex; flex-direction: column; gap: 4px; }
.m26-trace-spec-row { display: flex; align-items: center; gap: 6px; font-size: 11px; color: #64748b; }
.m26-trace-spec-row .label { color: #94a3b8; font-size: 11px; }
.m26-trace-spec-row .val { color: #334155; font-size: 11px; }
.m26-trace-spec-row .val-ttft { font-size: 10px; color: #0284c7; background: #e0f2fe; padding: 1px 5px; border-radius: 3px; font-family: ui-monospace, monospace; }
.m26-trace-error-box { margin-top: 6px; padding: 6px 8px; background: #fff1f2; border: 1px solid #ffe4e6; border-radius: 6px; display: flex; align-items: flex-start; gap: 6px; color: #e11d48; word-break: break-all; }
.m26-perf-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.m26-perf-card { padding: 8px 10px; background: #f8fafc; border: 1px solid var(--m26-border); border-radius: 8px; display: flex; flex-direction: column; gap: 3px; }
.m26-perf-card .val { font-size: 13px; font-weight: 700; color: var(--m26-text); }
@media (max-width: 640px) { .m26-diag-meta-grid, .m26-perf-grid { grid-template-columns: 1fr; } }
</style>
