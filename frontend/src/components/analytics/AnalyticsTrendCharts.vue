<script setup lang="ts">
import type { AnalyticsResult } from '@/services/gatewayApi'

const props = defineProps<{
  analyticsData: AnalyticsResult | null
  maxDailyRequests: number
  maxDailyTokens: number
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
</script>

<template>
  <div class="m26-analytics-charts-grid">
    <div class="m26-analytics-card">
      <div class="m26-card-header-sub">
        <div class="flex items-center gap-2"><div class="m26-card-sub-icon blue"></div><h3 class="text-sm font-semibold text-main">每日请求量走势</h3></div>
        <div class="flex items-center gap-3 text-xs text-muted"><span class="flex items-center gap-1.5"><span class="m26-legend-dot emerald"></span>成功</span><span class="flex items-center gap-1.5"><span class="m26-legend-dot rose"></span>失败</span></div>
      </div>
      <div v-if="props.analyticsData?.daily && props.analyticsData.daily.length > 0" class="m26-chart-container">
        <div class="m26-chart-bars-wrap">
          <div v-for="d in props.analyticsData.daily" :key="d.date" class="m26-bar-column-group">
            <div class="m26-bar-tooltip">
              <div class="font-semibold text-xs text-main mb-1">{{ d.date }}</div>
              <div class="text-xs text-muted flex justify-between gap-4"><span>总请求:</span><b class="text-main">{{ d.total_requests }} 次</b></div>
              <div class="text-xs text-muted flex justify-between gap-4"><span>成功:</span><span class="text-emerald-600 font-semibold">{{ d.success_requests }} 次</span></div>
              <div v-if="d.failed_requests" class="text-xs text-muted flex justify-between gap-4"><span>失败:</span><span class="text-rose-600 font-semibold">{{ d.failed_requests }} 次</span></div>
              <div class="text-xs text-muted flex justify-between gap-4 mt-1 pt-1 border-t border-slate-100"><span>均延 / 首字:</span><span class="font-mono text-main">{{ formatLatency(d.avg_latency_ms) }} / {{ formatLatency(d.avg_ttft_ms) }}</span></div>
            </div>
            <div class="m26-bar-track">
              <div v-if="d.failed_requests" class="m26-bar-fill failed" :style="{ height: Math.max((d.failed_requests / props.maxDailyRequests) * 100, 5) + '%' }"></div>
              <div class="m26-bar-fill success" :style="{ height: Math.max((d.success_requests / props.maxDailyRequests) * 100, 6) + '%' }"></div>
            </div>
            <span class="m26-bar-label">{{ d.date.length >= 10 ? d.date.slice(5) : d.date }}</span>
          </div>
        </div>
      </div>
      <div v-else class="m26-analytics-empty"><span>所选时间范围内暂无调用记录</span></div>
    </div>

    <div class="m26-analytics-card">
      <div class="m26-card-header-sub">
        <div class="flex items-center gap-2"><div class="m26-card-sub-icon purple"></div><h3 class="text-sm font-semibold text-main">每日 Token 消耗走势</h3></div>
        <div class="flex items-center gap-3 text-xs text-muted"><span class="flex items-center gap-1.5"><span class="m26-legend-dot purple"></span>提示 Prompt</span><span class="flex items-center gap-1.5"><span class="m26-legend-dot cyan"></span>生成 Completion</span></div>
      </div>
      <div v-if="props.analyticsData?.daily && props.analyticsData.daily.length > 0" class="m26-chart-container">
        <div class="m26-chart-bars-wrap">
          <div v-for="d in props.analyticsData.daily" :key="d.date" class="m26-bar-column-group">
            <div class="m26-bar-tooltip">
              <div class="font-semibold text-xs text-main mb-1">{{ d.date }}</div>
              <div class="text-xs text-muted flex justify-between gap-4"><span>总 Tokens:</span><b class="text-main">{{ formatTokens(d.total_tokens) }}</b></div>
              <div class="text-xs text-muted flex justify-between gap-4"><span>提示 Prompt:</span><span class="text-indigo-600 font-semibold">{{ formatTokens(d.prompt_tokens) }}</span></div>
              <div class="text-xs text-muted flex justify-between gap-4"><span>补全 Completion:</span><span class="text-cyan-600 font-semibold">{{ formatTokens(d.completion_tokens) }}</span></div>
            </div>
            <div class="m26-bar-track">
              <div class="m26-bar-fill purple" :style="{ height: Math.max((d.prompt_tokens / props.maxDailyTokens) * 100, 3) + '%' }"></div>
              <div class="m26-bar-fill cyan" :style="{ height: Math.max((d.completion_tokens / props.maxDailyTokens) * 100, 3) + '%' }"></div>
            </div>
            <span class="m26-bar-label">{{ d.date.length >= 10 ? d.date.slice(5) : d.date }}</span>
          </div>
        </div>
      </div>
      <div v-else class="m26-analytics-empty"><span>所选时间范围内暂无调用记录</span></div>
    </div>
  </div>
</template>
