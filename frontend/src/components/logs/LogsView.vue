<script setup lang="ts">
import type { RequestLog } from '@/services/gatewayApi'

const props = defineProps<{
  logs: RequestLog[]
  totalLogs: number
  search: string
  streamConnected: boolean
  autoScroll: boolean
  clearing: boolean
}>()

const emit = defineEmits<{
  (e: 'update:search', value: string): void
  (e: 'update:auto-scroll', value: boolean): void
  (e: 'clear'): void
  (e: 'open-detail', log: RequestLog): void
}>()

function formatDate(value: string): string {
  try { return new Date(value).toLocaleString('zh-CN', { hour12: false }) } catch { return value }
}
function duration(value?: number): string {
  if (!value || value <= 0) return '-'
  return value < 1000 ? `${value}ms` : `${(value / 1000).toFixed(2)}s`
}
function failoverNodes(value?: string): string[] {
  return (value || '').split(/\s*(?:➔|->|→)\s*/).map((item) => item.trim()).filter(Boolean)
}
</script>

<template>
  <section class="m26-section-view">
    <div class="m26-logs-header">
      <div class="m26-logs-left flex items-center gap-3">
        <span class="m26-logs-count">实时流水日志 <span class="m26-badge ml-1">{{ props.logs.length }}</span><span v-if="props.search" class="text-xs text-muted ml-2">(过滤自 {{ props.totalLogs }} 条)</span></span>
        <div class="m26-toolbar-search"><svg class="m26-search-icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><circle cx="6.5" cy="6.5" r="4.5" /><line x1="10" y1="10" x2="14" y2="14" /></svg><input :value="props.search" type="text" class="m26-toolbar-search-input" placeholder="搜索流水日志..." @input="emit('update:search', ($event.target as HTMLInputElement).value)" /><button v-if="props.search" class="m26-search-clear" title="清空搜索" @click="emit('update:search', '')">✕</button></div>
      </div>
      <div class="m26-logs-right">
        <div class="m26-stream-status-pill" :class="{ connected: props.streamConnected }" :title="props.streamConnected ? '本地实时事件推流已就绪' : '本地实时流连接中...'"><span class="m26-live-indicator"></span><span>{{ props.streamConnected ? '实时推流' : '连接中' }}</span></div>
        <button class="m26-glass-btn" :class="{ 'm26-btn-active-blue': props.autoScroll }" :title="props.autoScroll ? '点击暂停自动滚动' : '点击开启新日志自动滚动到顶'" @click="emit('update:auto-scroll', !props.autoScroll)">{{ props.autoScroll ? '📜 实时滚动：开' : '⏸ 实时滚动：关' }}</button>
        <button class="m26-glass-btn danger" :disabled="props.clearing" @click="emit('clear')">清空流水</button>
      </div>
    </div>

    <div v-if="props.logs.length" class="m26-table-wrap">
      <table class="m26-table font-mono text-xs">
<thead><tr><th style="width: 165px;" class="whitespace-nowrap">请求时间</th><th style="width: 95px;" class="whitespace-nowrap">状态</th><th style="width: 155px;" class="whitespace-nowrap">模型</th><th style="width: 220px;" class="whitespace-nowrap">供应商 / 轮转</th><th style="width: 130px;" class="whitespace-nowrap">用时 / 首字</th><th style="width: 75px;" class="whitespace-nowrap">提示</th><th style="width: 75px;" class="whitespace-nowrap">补全</th><th style="width: 65px;" class="text-right whitespace-nowrap">详情</th></tr></thead>
        <tbody>
<tr v-for="log in props.logs" :key="log.id" class="cursor-pointer transition-colors" :class="[log.status_code === 0 ? 'm26-row-in-flight' : 'hover:bg-slate-50/80']" title="点击查看请求与轮转详情" @click="emit('open-detail', log)">
          <td class="text-muted whitespace-nowrap">{{ formatDate(log.created_at) }}</td>
          <td class="whitespace-nowrap"><span class="m26-status-pill" :class="log.status_code === 0 ? 'status-streaming' : log.is_failover && log.status_code >= 200 && log.status_code < 300 ? 'status-failover' : log.status_code >= 200 && log.status_code < 300 ? 'status-200' : log.status_code === 429 ? 'status-429' : 'status-err'">{{ log.status_code === 0 ? '传输中' : log.is_failover && log.status_code >= 200 && log.status_code < 300 ? '轮转 200' : log.status_code >= 200 && log.status_code < 300 ? '200 OK' : log.status_code === 429 ? '429 限流' : (log.status_code + ' 异常') }}</span></td>
          <td><div class="flex flex-col items-start w-fit"><span class="m26-model-badge" :title="log.model"><span class="truncate max-w-[145px] font-mono">{{ log.model }}</span></span><span v-if="log.upstream_model && log.upstream_model !== log.model" class="text-[10px] text-slate-400 font-mono pl-1 pt-0.5 truncate max-w-[145px]" :title="'映射至上游: ' + log.upstream_model">➔ {{ log.upstream_model }}</span></div></td>
          <td><div v-if="log.is_failover && log.failover_from" class="m26-failover-flow" :title="log.failover_from + ' ➔ ' + log.channel_name"><template v-for="(node, index) in failoverNodes(log.failover_from)" :key="index"><span class="m26-tag-failover-from">{{ node }}</span><span class="m26-failover-arrow">➔</span></template><span class="m26-tag-failover-to">{{ log.channel_name }}</span></div><span v-else class="m26-channel-name-compact truncate block" :title="log.channel_name">{{ log.channel_name }}</span></td>
          <td class="whitespace-nowrap"><div class="flex items-center gap-1.5 flex-nowrap"><span class="m26-pill-total">{{ duration(log.latency_ms) }}</span><span class="m26-pill-ttft">{{ duration(log.ttft_ms || log.latency_ms) }}</span><span v-if="log.is_stream" class="m26-pill-stream">流</span></div></td>
          <td class="text-slate-600 font-mono text-xs whitespace-nowrap">{{ log.prompt_tokens ?? 0 }}</td><td class="whitespace-nowrap"><div class="flex items-center gap-1 font-mono text-xs text-slate-800 font-medium"><span v-if="log.status_code === 0" class="m26-token-pulse-dot"></span><span>{{ log.completion_tokens ?? 0 }}</span></div></td><td class="text-right whitespace-nowrap"><span class="m26-link-btn text-xs">详情 ➔</span></td>
        </tr>
</tbody>
      </table>
    </div>
    <div v-else class="m26-empty-state"><p v-if="props.search">没有匹配“{{ props.search }}”的请求流水。</p><p v-else>暂无网关请求流水。当第三方客户端向本地网关发起请求时将实时记录。</p></div>
  </section>
</template>
