<script setup lang="ts">
type RoutingStrategy = 'smart_quality' | 'latency' | 'round_robin'

const props = defineProps<{
  healthyChannels: number
  channelCount: number
  enabledChannelsCount: number
  averageLatency: number | null
  routingStrategy: RoutingStrategy
  modelCount: number
  availableModels: number
}>()

function formatLatencySec(ms: number | null | undefined): string {
  if (ms == null || isNaN(ms) || ms <= 0) return '—'
  const sec = ms / 1000
  return (sec >= 10 ? sec.toFixed(1) : sec.toFixed(2)) + 's'
}
</script>

<template>
  <div class="m26-bento-grid">
    <div class="m26-bento-card">
      <div class="m26-bento-head"><span class="m26-bento-label">可用渠道比率</span><span class="m26-pill-icon emerald"><svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor"><circle cx="8" cy="8" r="4" /></svg></span></div>
      <div class="m26-bento-stat"><span class="m26-bento-num">{{ props.healthyChannels }}</span><span class="m26-bento-sub">/ {{ props.channelCount }}</span></div>
      <div class="m26-bento-foot"><span class="m26-tag-subtle">{{ props.enabledChannelsCount }} 个启用调度中</span></div>
    </div>

    <div class="m26-bento-card">
      <div class="m26-bento-head"><span class="m26-bento-label">综合响应延迟</span><span class="m26-pill-icon indigo"><svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><polyline points="2 8 5 11 9 4 14 10" /></svg></span></div>
      <div class="m26-bento-stat"><span class="m26-bento-num">{{ formatLatencySec(props.averageLatency) }}</span></div>
      <div class="m26-bento-foot"><span class="m26-tag-subtle">{{ props.averageLatency && props.averageLatency < 500 ? '极速响应' : '正常负载' }}</span></div>
    </div>

    <div class="m26-bento-card">
      <div class="m26-bento-head"><span class="m26-bento-label">分发策略</span><span class="m26-pill-icon amber"><svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><circle cx="8" cy="8" r="6" /><line x1="8" y1="4" x2="8" y2="8" /><line x1="8" y1="8" x2="11" y2="11" /></svg></span></div>
      <div class="m26-bento-stat"><span class="m26-bento-text">{{ props.routingStrategy === 'smart_quality' ? '智能综合质量' : props.routingStrategy === 'latency' ? '最低延迟' : '轮询均衡' }}</span></div>
      <div class="m26-bento-foot"><span class="m26-tag-subtle">自动故障熔断隔离</span></div>
    </div>

    <div class="m26-bento-card">
      <div class="m26-bento-head"><span class="m26-bento-label">大模型总数</span><span class="m26-pill-icon cyan"><svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><polygon points="8 2 14 5 14 11 8 14 2 11 2 5" /></svg></span></div>
      <div class="m26-bento-stat"><span class="m26-bento-num">{{ props.modelCount }}</span><span class="m26-bento-sub">个聚合模型</span></div>
      <div class="m26-bento-foot"><span class="m26-tag-subtle">{{ props.availableModels }} 家上游提供</span></div>
    </div>
  </div>
</template>
