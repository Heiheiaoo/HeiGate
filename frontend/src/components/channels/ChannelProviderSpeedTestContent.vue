<script setup lang="ts">
import type { ModelProviderSpeedTestRow } from './types'

type ModelFamily = {
  label: string
  color: string
  bg: string
  border: string
}

const props = defineProps<{
  modelName: string
  modelFamily: ModelFamily
  providers: ModelProviderSpeedTestRow[]
  query: string
  fastest: { channelName: string; latency: number } | null
  probingAll: boolean
  copied: boolean
}>()

const emit = defineEmits<{
  (e: 'update:query', value: string): void
  (e: 'copy-model'): void
  (e: 'probe-all'): void
  (e: 'probe-provider', row: ModelProviderSpeedTestRow): void
  (e: 'open-diagnostic', row: ModelProviderSpeedTestRow): void
  (e: 'close'): void
}>()
</script>

<template>
  <div class="m26-modal-sheet m26-speedtest-modal-sheet" @click.stop>
    <div class="m26-modal-header">
      <div class="flex items-center gap-3">
        <div
          class="m26-model-brand-badge shrink-0"
          :style="{
            color: props.modelFamily.color,
            background: props.modelFamily.bg,
            borderColor: props.modelFamily.border,
          }"
        >
          <span class="m26-model-brand-dot" :style="{ background: props.modelFamily.color }"></span>
          <span class="m26-model-brand-name whitespace-nowrap">{{ props.modelFamily.label }}</span>
        </div>
        <div>
          <div class="flex items-center gap-2">
            <h3 class="m26-modal-title font-mono">{{ props.modelName }}</h3>
            <span class="m26-modal-count-pill">共 {{ props.providers.length }} 家供应商</span>
            <button class="m26-model-copy-btn" :title="'复制模型名: ' + props.modelName" @click.stop="emit('copy-model')">
              <svg v-if="!props.copied" viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                <rect x="5" y="5" width="8" height="8" rx="1.5" />
                <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
              </svg>
              <svg v-else viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="#059669" stroke-width="2.2" aria-hidden="true">
                <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
              </svg>
              <span>{{ props.copied ? '已复制' : '复制' }}</span>
            </button>
          </div>
          <p class="m26-modal-subtitle">同一模型多供应商并发探活 · 检验该模型在各渠道的端到端真实生成响应与延迟梯度</p>
        </div>
      </div>
      <button class="m26-close-btn" aria-label="关闭" @click="emit('close')">✕</button>
    </div>

    <div class="m26-modal-toolbar">
      <div class="m26-modal-search">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <circle cx="6.5" cy="6.5" r="4.5" />
          <line x1="10" y1="10" x2="14" y2="14" />
        </svg>
        <input
          :value="props.query"
          type="text"
          placeholder="搜索供应商名称、上游别名或接口地址..."
          class="m26-modal-search-input"
          @input="emit('update:query', ($event.target as HTMLInputElement).value)"
        />
      </div>

      <div class="flex items-center gap-2">
        <div v-if="props.fastest" class="m26-fastest-provider-pill">
          <span class="m26-fastest-dot"></span>
          <span>⚡ 最优: {{ props.fastest.channelName }} (首字 {{ props.fastest.latency < 1000 ? `${props.fastest.latency}ms` : `${(props.fastest.latency / 1000).toFixed(2)}s` }})</span>
        </div>
        <button
          class="m26-action-btn primary"
          :disabled="props.probingAll"
          title="重新对该模型的所有供应商发起并发真实探活"
          @click="emit('probe-all')"
        >
          <svg v-if="!props.probingAll" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
          </svg>
          <span v-else class="m26-spin-dot"></span>
          <span>{{ props.probingAll ? '全渠道并发探活中...' : '重新探活全部供应商' }}</span>
        </button>
      </div>
    </div>

    <div class="m26-modal-body m26-speedtest-body">
      <div v-if="props.providers.length > 0" class="m26-model-grid">
        <div
          v-for="(row, idx) in props.providers"
          :key="row.channel.gatewayId || row.channel.name"
          class="m26-model-item-card"
          :class="{ 'is-preferred': row.preferred }"
        >
          <div class="m26-model-item-info">
            <div class="flex items-center gap-2 flex-wrap">
              <span class="m26-model-idx">#{{ idx + 1 }}</span>
              <span class="m26-channel-name font-semibold">{{ row.channel.name }}</span>
              <span class="m26-proto-pill" :class="row.channel.protocol">{{ row.protocolLabel }}</span>
              <span v-if="row.preferred" class="m26-tag-preferred">⚡ 优选首选 (网关优先调度)</span>
              <span v-if="!row.channel.enabled" class="m26-tag-disabled">已暂停</span>
            </div>
            <div class="flex items-center gap-2 mt-1 text-xs text-slate-500 dark:text-slate-400 flex-wrap">
              <span class="m26-speedtest-alias-tag">上游别名: <code class="font-mono text-indigo-600 dark:text-indigo-400">{{ row.upstreamModel }}</code></span>
              <span class="m26-model-ch-url" :title="row.channel.url">{{ row.channel.url }}</span>
            </div>
          </div>

          <div class="m26-model-item-actions">
            <div
              class="m26-model-lat-pill"
              :class="[row.latencyClass, { 'has-err-link': row.isFailed }]"
              :title="row.tooltip"
              @click="row.isFailed && emit('open-diagnostic', row)"
            >
              <span class="lat-dot"></span>
              <span class="lat-text">{{ row.latencyDisplay }}</span>
              <svg v-if="row.isFailed" class="lat-view-err" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                <circle cx="8" cy="8" r="6" />
                <path d="M8 4.5v4M8 11.3v.2" />
              </svg>
            </div>
            <button class="m26-btn-micro probe" :disabled="row.isProbing" title="单独重新探活此供应商" @click="emit('probe-provider', row)">
              <span v-if="row.isProbing" class="m26-spin-dot"></span>
              <span>{{ row.isProbing ? '探活中' : '探活' }}</span>
            </button>
            <button class="m26-btn-icon" title="查看此渠道完整诊断信息" @click="emit('open-diagnostic', row)">
              <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                <circle cx="8" cy="8" r="6" />
                <path d="M8 7v4M8 5h.01" />
              </svg>
            </button>
          </div>
        </div>
      </div>
      <div v-else class="m26-empty-state py-8 text-center text-slate-400">
        <p v-if="props.query">未找到匹配 “{{ props.query }}” 的供应商渠道</p>
        <p v-else>暂无支持该模型的供应商渠道</p>
      </div>
    </div>

    <div class="m26-modal-footer">
      <span class="m26-modal-hint text-xs text-slate-400">💡 网关调度规则：本地请求该模型时，HeiGate 将首选上方延迟最低的健康供应商（带 ⚡ 优选首选 标识）。若单次调用遇到异常或限流，将执行 1 次就地快速重试，仍失败则自动无缝轮转至次优供应商，全方位保障高可用。</span>
    </div>
  </div>
</template>
