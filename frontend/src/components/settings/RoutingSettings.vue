<script setup lang="ts">
import type { GlobalRouteStrategy } from '@/views/GatewayView.vue'

defineProps<{ routingStrategy: GlobalRouteStrategy }>()
const emit = defineEmits<{ select: [strategy: GlobalRouteStrategy] }>()
</script>

<template>
  <div class="m26-pref-group">
    <div class="m26-pref-group-header">
      <div class="m26-pref-group-icon icon-route">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="18" cy="18" r="3" />
          <circle cx="6" cy="6" r="3" />
          <path d="M13 6h3a2 2 0 0 1 2 2v7" />
          <line x1="6" y1="9" x2="6" y2="21" />
        </svg>
      </div>
      <div class="m26-pref-group-titles">
        <h3 class="m26-pref-title">分发与路由引擎</h3>
        <span class="m26-pref-subtitle">配置网关收到客户端请求时的多通道调度算法与熔断顺延机制</span>
      </div>
    </div>

    <div class="m26-route-cards">
      <!-- Card 1: Smart Quality -->
      <div
        class="m26-route-card"
        :class="{ selected: routingStrategy === 'smart_quality' }"
        @click="emit('select', 'smart_quality')"
      >
        <div class="m26-route-card-head">
          <div class="m26-route-type-badge smart_quality">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
            </svg>
          </div>
          <div class="m26-route-check-indicator">
            <svg v-if="routingStrategy === 'smart_quality'" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          </div>
        </div>
        <div class="m26-route-body">
          <div class="m26-route-name-wrap">
            <span class="m26-route-name">智能综合质量</span>
            <span class="m26-route-code-tag">Smart Quality</span>
          </div>
          <p class="m26-route-desc">综合就绪状态、实时延迟与历史稳定性动态评分，遇 429 限流或失败自动降权避让。</p>
        </div>
      </div>

      <!-- Card 2: Lowest Latency -->
      <div
        class="m26-route-card"
        :class="{ selected: routingStrategy === 'latency' }"
        @click="emit('select', 'latency')"
      >
        <div class="m26-route-card-head">
          <div class="m26-route-type-badge latency">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
            </svg>
          </div>
          <div class="flex items-center gap-2">
            <span class="m26-route-pill-rec">推荐</span>
            <div class="m26-route-check-indicator">
              <svg v-if="routingStrategy === 'latency'" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3">
                <polyline points="20 6 9 17 4 12" />
              </svg>
            </div>
          </div>
        </div>
        <div class="m26-route-body">
          <div class="m26-route-name-wrap">
            <span class="m26-route-name">最低延迟优先</span>
            <span class="m26-route-code-tag">Lowest Latency</span>
          </div>
          <p class="m26-route-desc">实时选取探活延迟最低的可用通道发起调用，提供极致响应速度与流畅体感。</p>
        </div>
      </div>

      <!-- Card 3: Round Robin -->
      <div
        class="m26-route-card"
        :class="{ selected: routingStrategy === 'round_robin' }"
        @click="emit('select', 'round_robin')"
      >
        <div class="m26-route-card-head">
          <div class="m26-route-type-badge roundrobin">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67" />
            </svg>
          </div>
          <div class="m26-route-check-indicator">
            <svg v-if="routingStrategy === 'round_robin'" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          </div>
        </div>
        <div class="m26-route-body">
          <div class="m26-route-name-wrap">
            <span class="m26-route-name">轮询均衡</span>
            <span class="m26-route-code-tag">Round Robin</span>
          </div>
          <p class="m26-route-desc">在所有正常状态的通道中均匀分配流量，防止单个账号频繁调用限流。</p>
        </div>
      </div>
    </div>
  </div>
</template>

