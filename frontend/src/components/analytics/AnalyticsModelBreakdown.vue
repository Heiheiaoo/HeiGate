<script setup lang="ts">
import type { ModelAnalyticsStat } from '@/services/gatewayApi'

const props = defineProps<{
  models: ModelAnalyticsStat[]
  totalRequests: number
  search: string
}>()

const emit = defineEmits<{
  (e: 'update:search', value: string): void
}>()

function formatTokens(value: number): string {
  if (!value || isNaN(value)) return '0'
  if (value >= 1000000) return (value / 1000000).toFixed(2) + 'M'
  if (value >= 1000) return (value / 1000).toFixed(1) + 'K'
  return value.toLocaleString()
}

function formatLatency(value: number): string {
  if (!value || isNaN(value) || value <= 0) return '-'
  const sec = value / 1000
  return (sec >= 10 ? sec.toFixed(1) : sec.toFixed(2)) + 's'
}

function latencyClass(value: number): string {
  if (!value || value <= 0) return 'text-muted'
  if (value < 1000) return 'm26-tag-fast'
  if (value < 3000) return 'm26-tag-normal'
  return 'm26-tag-slow'
}
</script>

<template>
  <div class="m26-analytics-card">
    <div class="m26-card-header-sub">
      <div>
        <h3 class="text-sm font-semibold text-main">各模型用量与时延表现明细</h3>
        <p class="text-xs text-muted mt-0.5">按调用量降序分析各模型的 Token 规模、端到端延迟、首字 TTFT 与首选承载渠道</p>
      </div>
      <div class="m26-model-filter-input-wrap">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2"><circle cx="6.5" cy="6.5" r="4.5" /><line x1="10" y1="10" x2="14" y2="14" /></svg>
        <input :value="props.search" type="text" placeholder="搜索模型或承载渠道..." class="m26-sub-search-input" @input="emit('update:search', ($event.target as HTMLInputElement).value)" />
      </div>
    </div>

    <div v-if="props.models.length > 0" class="m26-analytics-table-wrap">
      <table class="m26-analytics-table">
        <thead><tr><th class="text-left" style="width: 25%;">模型名称</th><th class="text-left" style="width: 18%;">调用频次 & 占比</th><th class="text-left" style="width: 18%;">Token 消耗 (总 / 提 / 补)</th><th class="text-left" style="width: 13%;">平均耗时</th><th class="text-left" style="width: 12%;">首字用时</th><th class="text-left" style="width: 14%;">时延范围 (最小~最大)</th></tr></thead>
        <tbody>
          <tr v-for="model in props.models" :key="model.model" class="m26-analytics-row">
            <td><div class="flex items-center gap-2.5"><div class="m26-model-icon-dot"></div><div><div class="font-semibold text-xs text-main font-mono">{{ model.model }}</div><div v-if="model.top_channel" class="text-[10.5px] text-muted flex items-center gap-1 mt-0.5"><span>主承载:</span><span class="m26-channel-tag">{{ model.top_channel }}</span></div></div></div></td>
            <td><div><div class="flex items-center justify-between text-xs font-mono mb-1"><span class="text-main font-semibold">{{ model.total_requests }} 次</span><span class="text-muted text-[11px]">{{ ((model.total_requests / (props.totalRequests || 1)) * 100).toFixed(1) }}%</span></div><div class="m26-mini-progress-bar"><div class="m26-mini-progress-fill" :style="{ width: Math.min((model.total_requests / (props.totalRequests || 1)) * 100, 100) + '%' }"></div></div></div></td>
            <td><div class="text-xs"><div class="font-semibold text-main font-mono">{{ formatTokens(model.total_tokens) }}</div><div class="text-[10.5px] text-muted font-mono mt-0.5">提 {{ formatTokens(model.prompt_tokens) }} · 补 {{ formatTokens(model.completion_tokens) }}</div></div></td>
            <td><span class="m26-latency-pill font-mono" :class="latencyClass(model.avg_latency_ms)">{{ formatLatency(model.avg_latency_ms) }}</span></td>
            <td><span class="text-xs font-mono font-medium text-main">{{ formatLatency(model.avg_ttft_ms) }}</span></td>
            <td><span class="text-xs font-mono text-muted">{{ formatLatency(model.min_latency_ms) }} ~ {{ formatLatency(model.max_latency_ms) }}</span></td>
          </tr>
        </tbody>
      </table>
    </div>
    <div v-else class="m26-analytics-empty"><span>无匹配的模型记录</span></div>
  </div>
</template>
