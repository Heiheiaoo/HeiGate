<script setup lang="ts">
import type { ChannelAnalyticsStat } from '@/services/gatewayApi'

const props = defineProps<{
  channels: ChannelAnalyticsStat[]
}>()

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
  <div v-if="props.channels.length > 0" class="m26-analytics-card">
    <div class="m26-card-header-sub">
      <div>
        <h3 class="text-sm font-semibold text-main">上游供应商渠道调度与承载分布</h3>
        <p class="text-xs text-muted mt-0.5">统计网关向各个渠道分发请求的吞吐量、平均响应耗时及成功率表现</p>
      </div>
    </div>
    <div class="m26-channel-perf-grid">
      <div v-for="channel in props.channels" :key="channel.channel_name" class="m26-channel-perf-card">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2"><span class="m26-channel-dot"></span><span class="font-semibold text-xs text-main">{{ channel.channel_name }}</span></div>
          <span v-if="channel.success_rate >= 95" class="m26-badge emerald text-[11px]">{{ channel.success_rate.toFixed(0) }}% 成功</span>
          <span v-else class="m26-badge amber text-[11px]">{{ channel.success_rate.toFixed(0) }}% 成功</span>
        </div>
        <div class="grid grid-cols-3 gap-2 mt-3 pt-3 border-t border-slate-100">
          <div><span class="text-[10.5px] text-muted block">承载请求</span><span class="font-semibold text-xs text-main font-mono">{{ channel.total_requests }} 次</span></div>
          <div><span class="text-[10.5px] text-muted block">平均耗时</span><span class="font-semibold text-xs font-mono" :class="latencyClass(channel.avg_latency_ms)">{{ formatLatency(channel.avg_latency_ms) }}</span></div>
          <div><span class="text-[10.5px] text-muted block">首字用时</span><span class="font-semibold text-xs text-main font-mono">{{ formatLatency(channel.avg_ttft_ms) }}</span></div>
        </div>
      </div>
    </div>
  </div>
</template>
