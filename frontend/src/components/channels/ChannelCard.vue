<script setup lang="ts">
import { computed } from 'vue'
import type { Channel } from './types'

const props = defineProps<{
  channel: Channel
  isSelected?: boolean
}>()

const emit = defineEmits<{
  (e: 'select', c: Channel): void
  (e: 'toggle', c: Channel): void
  (e: 'probe', c: Channel): void
  (e: 'edit', c: Channel): void
  (e: 'duplicate', c: Channel): void
  (e: 'remove', c: Channel): void
  (e: 'open-models', c: Channel): void
  (e: 'open-diagnostic', c: Channel): void
  (e: 'open-inject', c: Channel): void
}>()

const c = computed(() => props.channel)

function formatLatencySec(ms: number | null | undefined): string {
  if (ms == null) return '超时'
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(2)}s`
}


function statusBadgeClass(status: string): string {
  switch (status) {
    case 'healthy': return 'status-healthy'
    case 'degraded': return 'status-degraded'
    case 'unknown': return 'status-unknown'
    default: return 'status-error'
  }
}

function statusText(status: string): string {
  switch (status) {
    case 'healthy': return '就绪'
    case 'degraded': return '高延迟'
    case 'unknown': return '待探活'
    default: return '异常'
  }
}

function injectionClass(state?: string): string {
  switch (state) {
    case 'clean': return 'inject-clean'
    case 'suspected': return 'inject-suspected'
    case 'detected': return 'inject-detected'
    default: return 'inject-unknown'
  }
}

function injectionLabel(state?: string): string {
  switch (state) {
    case 'clean': return '输入正常'
    case 'suspected': return '输入偏高'
    case 'detected': return '输入显著偏高'
    case 'error': return '检测异常'
    default: return '未检测'
  }
}

const isCircuitOpen = computed(() => {
  return c.value.circuitState && c.value.circuitState !== 'closed'
})

const modelCount = computed(() => c.value.modelNames?.length ?? 0)

function onCardClick() {
  if (window.getSelection()?.toString().trim()) return
  emit('select', c.value)
}

function onModelHighlightClick() {
  if (window.getSelection()?.toString().trim()) return
  emit('open-models', c.value)
}
</script>

<template>
  <div
    class="m26-channel-card"
    :class="[
      c.status,
      {
        'is-disabled': !c.enabled,
        'is-selected': isSelected,
        'is-circuit-open': isCircuitOpen,
      }
    ]"
    tabindex="0"
    @click="onCardClick"
  >
    <!-- Top Row: Name, Status Pill, and Toggle Switch -->
    <div class="m26-card-header">
      <div class="m26-card-title-box">
        <h4 class="m26-card-title" :title="`${c.name}\n协议: ${c.protocol}\n${c.url}`">{{ c.name }}</h4>
        <span v-if="c.keyCount" class="text-xs text-muted">{{ c.keyCount }} Key · {{ c.keyAvailableCount }} 可用<span v-if="c.keyCoolingCount"> · {{ c.keyCoolingCount }} 冷却</span></span>
      </div>

      <div class="m26-card-ctrl-box" @click.stop>
        <!-- Status Indicator Pill -->
        <span
          class="m26-card-status"
          :class="statusBadgeClass(c.status)"
          :title="c.lastError ? `探活主模型 [${c.primaryModel || '默认'}] 状态: ${statusText(c.status)} (${c.lastError})` : `探活主模型 [${c.primaryModel || '默认'}] 状态: ${statusText(c.status)}`"
          @click="c.status !== 'healthy' && c.lastError ? emit('open-diagnostic', c) : null"
        >
          <span class="status-dot"></span>
          <span>{{ statusText(c.status) }}</span>
        </span>

        <!-- Toggle Switch -->
        <label class="m26-switch-label" :title="c.enabled ? '点击停用渠道' : '点击启用渠道'">
          <input
            type="checkbox"
            :checked="c.enabled"
            class="m26-switch-input"
            @change="emit('toggle', c)"
          />
          <span class="m26-switch-slider"></span>
        </label>
      </div>
    </div>

    <!-- Warning Strip if Circuit is Open or Cooling Down -->
    <div v-if="isCircuitOpen" class="m26-circuit-strip">
      <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" class="shrink-0">
        <path d="M8 2v8M8 13v1" />
      </svg>
      <span>熔断冷却保护中 (连续失败 {{ c.failureCount }} 次)</span>
    </div>

    <!-- Middle Body: Primary Model -->
    <div
      class="m26-model-highlight"
      title="点击打开此渠道模型面板与实时探活"
      @click.stop="onModelHighlightClick"
    >
      <div class="m26-model-info min-w-0">
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" class="m26-model-icon shrink-0">
          <rect x="2.5" y="2.5" width="11" height="11" rx="3" />
          <path d="M5.5 8h5M8 5.5v5" stroke-linecap="round" />
        </svg>
        <span class="m26-model-name font-mono truncate" :title="`探活主模型: ${c.primaryModel || '默认模型'}`">
          {{ c.primaryModel || '默认模型' }}
        </span>
      </div>

      <!-- Models count chip / Single model probe chip -->
      <button
        v-if="modelCount > 1"
        class="m26-models-count-chip"
        title="点击查看全部支持模型及探活"
        @click.stop="emit('open-models', c)"
      >
        {{ modelCount }} 模型
      </button>
      <button
        v-else
        class="m26-models-count-chip single icon-only"
        title="打开模型管理与探活"
        aria-label="打开模型管理与探活"
        @click.stop="emit('open-models', c)"
      >
        <svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
          <path d="M2 10h3l2-5 4 10 2-5h5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
    </div>

    <!-- Unified Bottom Row: Metrics on Left, Actions on Right -->
    <div class="m26-card-bottom" @click.stop>
      <!-- Metrics (Latency / Error & Injection) -->
      <div class="m26-bottom-metrics">
        <!-- Error Pill -->
        <button
          v-if="c.status !== 'healthy' && c.lastError"
          class="m26-metric-pill error icon-only"
          :title="`查看主模型报错排障建议: ${c.lastError}`"
          :aria-label="`查看主模型报错排障建议: ${c.lastError}`"
          @click.stop="emit('open-diagnostic', c)"
        >
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
            <circle cx="8" cy="8" r="6" />
            <path d="M8 4.5v4M8 11.3v.2" />
          </svg>
        </button>

        <!-- Latency Pill -->
        <div
          v-else
          class="m26-metric-pill latency"
          :class="{ degraded: c.status === 'degraded' }"
          :title="`探活主模型 [${c.primaryModel || '默认'}] 首字响应 (TTFT): ${formatLatencySec(c.ttft ?? c.latency)}`"
        >
          <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2" class="metric-icon">
            <path d="M9 1.5L3.5 8.5H8L7 14.5L12.5 7.5H8L9 1.5Z" stroke-linejoin="round" />
          </svg>
          <span class="font-mono">{{ formatLatencySec(c.ttft ?? c.latency) }}</span>
        </div>

        <!-- Injection Shield Pill -->
        <button
          class="m26-metric-pill inject"
          :class="injectionClass(c.injectionState)"
          :title="`提示词注入检测: ${injectionLabel(c.injectionState)} (点击重新检测)`"
          @click.stop="emit('open-inject', c)"
        >
          <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.8" class="inject-shield-icon">
            <path d="M8 1.5l5 2v4.5c0 3.5-3 6-5 7-2-1-5-3.5-5-7V3.5l5-2z" />
          </svg>
          <span>{{ injectionLabel(c.injectionState) }}</span>
        </button>
      </div>

      <!-- Action Buttons Group (Ghost Minimalist: Edit and Delete only) -->
      <div class="m26-action-btns">
        <button
          class="m26-card-act-btn"
          title="编辑渠道配置"
          @click="emit('edit', c)"
        >
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8">
            <path d="M11 2l3 3-8 8H3v-3l8-8z" />
          </svg>
        </button>

        <button
          class="m26-card-act-btn danger"
          title="删除此渠道"
          @click="emit('remove', c)"
        >
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8">
            <polyline points="3 4 5 4 13 4" />
            <path d="M5 4v8a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V4" />
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.m26-channel-card {
  position: relative;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.07);
  border-radius: 12px;
  padding: 13px 15px 11px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.03), 0 3px 10px rgba(0, 0, 0, 0.015);
  transition: all 0.18s cubic-bezier(0.16, 1, 0.3, 1);
  cursor: pointer;
  user-select: none;
  min-height: 122px;
}

.m26-channel-card:hover {
  transform: translateY(-2px);
  border-color: rgba(0, 0, 0, 0.13);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.06), 0 2px 6px rgba(0, 0, 0, 0.03);
}

.m26-channel-card.is-selected {
  border-color: #3b82f6;
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.25), 0 6px 20px rgba(59, 130, 246, 0.08);
}

.m26-channel-card.is-disabled {
  opacity: 0.55;
  filter: grayscale(40%);
  background: #fafafa;
}

.m26-channel-card.is-circuit-open {
  border-color: rgba(245, 158, 11, 0.45);
  background: #fffdf5;
}

/* Header */
.m26-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 10px;
}

.m26-card-title-box {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  flex: 1;
}

.m26-card-title {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: #0f172a;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  letter-spacing: -0.1px;
  user-select: text !important;
  -webkit-user-select: text !important;
  cursor: text;
}


.m26-card-ctrl-box {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.m26-card-status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 10.5px;
  font-weight: 500;
  padding: 1.5px 7px;
  border-radius: 9999px;
  cursor: default;
  transition: opacity 0.12s;
}

.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}

.status-healthy {
  background: #ecfdf5;
  color: #059669;
  border: 1px solid rgba(5, 150, 105, 0.12);
}
.status-healthy .status-dot {
  background: #10b981;
  box-shadow: 0 0 5px rgba(16, 185, 129, 0.6);
}

.status-degraded {
  background: #fffbeb;
  color: #d97706;
  border: 1px solid rgba(217, 119, 6, 0.12);
}
.status-degraded .status-dot {
  background: #f59e0b;
}

.status-error {
  background: #fef2f2;
  color: #dc2626;
  border: 1px solid rgba(220, 38, 38, 0.12);
  cursor: pointer;
}
.status-error:hover {
  background: #fee2e2;
}
.status-error .status-dot {
  background: #ef4444;
}

.status-unknown {
  background: #f8fafc;
  color: #64748b;
  border: 1px solid rgba(100, 116, 139, 0.15);
}
.status-unknown .status-dot {
  background: #94a3b8;
}

/* Switch */
.m26-switch-label {
  position: relative;
  display: inline-block;
  width: 28px;
  height: 16px;
  cursor: pointer;
}

.m26-switch-input {
  opacity: 0;
  width: 0;
  height: 0;
}

.m26-switch-slider {
  position: absolute;
  inset: 0;
  background-color: #cbd5e1;
  border-radius: 16px;
  transition: 0.15s;
}

.m26-switch-slider::before {
  position: absolute;
  content: "";
  height: 12px;
  width: 12px;
  left: 2px;
  bottom: 2px;
  background-color: white;
  border-radius: 50%;
  transition: 0.15s;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.15);
}

.m26-switch-input:checked + .m26-switch-slider {
  background-color: #10b981;
}

.m26-switch-input:checked + .m26-switch-slider::before {
  transform: translateX(12px);
}

/* Circuit Warning Strip */
.m26-circuit-strip {
  display: flex;
  align-items: center;
  gap: 6px;
  background: #fffbeb;
  border: 1px solid rgba(245, 158, 11, 0.25);
  padding: 4px 9px;
  border-radius: 6px;
  font-size: 10.5px;
  font-weight: 500;
  color: #b45309;
  margin-bottom: 10px;
}

.m26-model-highlight {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  background: #f8fafc;
  padding: 5px 9px;
  border-radius: 7px;
  border: 1px solid #f1f5f9;
  cursor: pointer;
  transition: background 0.12s;
  margin-bottom: 9px;
}

.m26-model-highlight:hover {
  background: #f1f5f9;
}

.m26-model-info {
  display: flex;
  align-items: center;
  gap: 6px;
}

.m26-model-icon {
  color: #6366f1;
}

.m26-model-name {
  font-size: 11.5px;
  font-weight: 500;
  color: #334155;
  user-select: text !important;
  -webkit-user-select: text !important;
  cursor: text;
}

.m26-models-count-chip {
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 500;
  padding: 1.5px 7px;
  border-radius: 9999px;
  background: #ffffff;
  color: #64748b;
  border: 1px solid rgba(0, 0, 0, 0.06);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  cursor: pointer;
  transition: all 0.12s;
}

.m26-models-count-chip:hover {
  background: #eff6ff;
  color: #2563eb;
  border-color: rgba(37, 99, 235, 0.2);
}

.m26-models-count-chip.single {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: #6366f1;
  background: rgba(99, 102, 241, 0.06);
  border: 1px solid rgba(99, 102, 241, 0.18);
  padding: 1.5px 7px;
  font-weight: 600;
}

.m26-models-count-chip.single.icon-only {
  width: 27px;
  height: 24px;
  justify-content: center;
  padding: 0;
}

.m26-models-count-chip.single:hover {
  background: rgba(99, 102, 241, 0.14);
  color: #4f46e5;
  border-color: rgba(99, 102, 241, 0.35);
}

.m26-metric-pill {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 5px;
  cursor: default;
  transition: all 0.12s;
  line-height: 1.2;
}

.m26-metric-pill.latency {
  background: #ecfdf5;
  color: #047857;
  border: 1px solid rgba(4, 120, 87, 0.12);
}

.m26-metric-pill.latency.degraded {
  background: #fffbeb;
  color: #b45309;
  border: 1px solid rgba(180, 83, 9, 0.12);
}

.m26-metric-pill.error {
  background: #fef2f2;
  color: #dc2626;
  border: 1px solid rgba(220, 38, 38, 0.18);
  cursor: pointer;
  font-weight: 500;
}

.m26-metric-pill.error.icon-only {
  width: 24px;
  height: 24px;
  justify-content: center;
  padding: 0;
}

.m26-metric-pill.error:hover {
  background: #fee2e2;
  border-color: rgba(220, 38, 38, 0.3);
}

.metric-dot.red {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #ef4444;
}

.m26-metric-pill.probe-mode {
  background: #f1f5f9;
  color: #475569;
  border: 1px solid #e2e8f0;
  cursor: pointer;
  user-select: none;
  font-weight: 500;
}

.m26-metric-pill.probe-mode:hover {
  background: #e2e8f0;
  color: #1e293b;
}

.m26-metric-pill.probe-mode.periodic {
  background: rgba(99, 102, 241, 0.08);
  color: #4f46e5;
  border: 1px solid rgba(99, 102, 241, 0.2);
}

.m26-metric-pill.probe-mode.periodic:hover {
  background: rgba(99, 102, 241, 0.14);
}

.m26-metric-pill.inject {
  cursor: pointer;
  transition: all 0.12s;
}

.inject-clean {
  color: #475569;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
}
.inject-clean .inject-shield-icon {
  color: #10b981;
}
.inject-clean:hover {
  background: #f1f5f9;
  color: #0f172a;
}

.inject-suspected {
  color: #b45309;
  background: #fffbeb;
  border: 1px solid rgba(217, 119, 6, 0.22);
  font-weight: 600;
}
.inject-suspected .inject-shield-icon {
  color: #f59e0b;
}
.inject-suspected:hover {
  background: #fef3c7;
}

.inject-detected {
  color: #dc2626;
  background: #fef2f2;
  border: 1px solid rgba(220, 38, 38, 0.25);
  font-weight: 600;
}
.inject-detected .inject-shield-icon {
  color: #ef4444;
}
.inject-detected:hover {
  background: #fee2e2;
}

.inject-unknown {
  color: #94a3b8;
  background: transparent;
  border: 1px dashed #cbd5e1;
}
.inject-unknown:hover {
  background: #f8fafc;
  color: #64748b;
}

/* Bottom Row: Left Metrics & Right Action Icons */
.m26-card-bottom {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid rgba(0, 0, 0, 0.05);
  margin-top: auto;
}

.m26-bottom-metrics {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  flex-wrap: wrap;
}

.m26-action-btns {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

.m26-card-act-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 5px;
  color: #64748b;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-card-act-btn:hover:not(:disabled) {
  background: rgba(0, 0, 0, 0.05);
  color: #0f172a;
}

.m26-card-act-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.m26-card-act-btn.danger:hover:not(:disabled) {
  background: #fef2f2;
  color: #dc2626;
}

.m26-spin-dot.mini {
  width: 10px;
  height: 10px;
  border: 1.5px solid rgba(59, 130, 246, 0.3);
  border-top-color: #3b82f6;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
