<script setup lang="ts">
import type { AggregateModelAliasItem, AggregateModelChannelDetail, AggregateModelView } from './types'

type ModelFamily = {
  label: string
  color: string
  bg: string
  border: string
}

const props = defineProps<{
  model: AggregateModelView
  family: ModelFamily
  aliases: AggregateModelAliasItem[]
  channelDetails: AggregateModelChannelDetail[]
  search: string
  copiedText: string
  mappedAliases: Record<string, string>
}>()

const emit = defineEmits<{
  (e: 'update:search', value: string): void
  (e: 'copy-model'): void
  (e: 'copy-aliases'): void
  (e: 'copy-alias', value: string): void
  (e: 'remove-mapping', value: string): void
  (e: 'open-speed-test'): void
  (e: 'open-classify'): void
  (e: 'close'): void
}>()

function latency(value: number | null): string {
  if (value == null || value <= 0) return ''
  return value < 1000 ? `${value}ms` : `${(value / 1000).toFixed(2)}s`
}

function statusLabel(status: AggregateModelChannelDetail['status']): string {
  return status === 'healthy' ? '正常' : status === 'degraded' ? '延迟较高' : '离线'
}
</script>

<template>
  <div class="m26-modal-sheet m26-aliases-modal-sheet">
    <div class="m26-modal-header">
      <div class="flex items-center gap-3">
        <span class="m26-model-family-badge" :style="{ color: props.family.color, background: props.family.bg, borderColor: props.family.border }">
          <span class="m26-badge-dot" :style="{ background: props.family.color }"></span>
          <span>{{ props.family.label }}</span>
        </span>
        <div>
          <div class="flex items-center gap-2">
            <h3 class="m26-modal-title font-mono">{{ props.model.name }}</h3>
            <button class="m26-model-copy-btn" :title="'复制模型名: ' + props.model.name" @click.stop="emit('copy-model')">
              <svg v-if="props.copiedText !== props.model.name" viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="5" y="5" width="8" height="8" rx="1.5" /><path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" /></svg>
              <svg v-else viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="#059669" stroke-width="2.2" aria-hidden="true"><polyline points="13.5 4.5 6.5 11.5 2.5 7.5" /></svg>
              <span>{{ props.copiedText === props.model.name ? '已复制' : '复制' }}</span>
            </button>
          </div>
          <p class="m26-modal-subtitle">统一网关模型 · 聚合 {{ props.model.aliases.length }} 个上游别名 · 共 {{ props.model.providerCount }} 个供货渠道</p>
        </div>
      </div>
      <button class="m26-close-btn" aria-label="关闭" @click="emit('close')">✕</button>
    </div>

    <div class="m26-modal-toolbar">
      <div class="m26-modal-search">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="6.5" cy="6.5" r="4.5" /><line x1="10" y1="10" x2="14" y2="14" /></svg>
        <input :value="props.search" type="text" placeholder="搜索别名或渠道名称..." class="m26-modal-search-input" @input="emit('update:search', ($event.target as HTMLInputElement).value)" />
      </div>
      <div class="flex items-center gap-2">
        <button class="m26-action-btn secondary" title="并发探活此模型全部供应商的可用性与延迟" @click="emit('open-speed-test')">探活全部供应商</button>
        <button class="m26-action-btn secondary" title="将其他渠道模型手动归并到此聚合模型" @click="emit('open-classify')">归入新别名</button>
        <button v-if="props.model.aliases.length > 0" class="m26-action-btn secondary" title="复制全部上游别名（每行一个）" @click="emit('copy-aliases')">
          {{ props.copiedText === '__ALL__' ? '已复制全部' : '复制全部别名' }}
        </button>
      </div>
    </div>

    <div class="m26-modal-body m26-alias-modal-body">
      <div class="m26-alias-guide-card">
        <div class="m26-alias-guide-header"><div class="flex items-center gap-2"><span class="m26-guide-badge">外部调用规范</span><span class="text-xs text-slate-500">外部应用调用此聚合模型时请求参数填写：</span></div></div>
        <div class="m26-alias-guide-code font-mono"><span class="text-indigo-600 font-semibold">"model"</span>: <span class="text-emerald-700">"{{ props.model.name }}"</span></div>
      </div>

      <div class="m26-alias-section">
        <div class="m26-alias-section-head"><span class="m26-alias-section-title">聚合的上游原始别名 ({{ props.aliases.length }})</span><span class="text-xs text-slate-400">各渠道实际声明的上游模型标识</span></div>
        <div v-if="props.aliases.length === 0" class="m26-empty-alias-tip">该模型无额外上游别名，所有提供渠道均直接使用标准名 <code>{{ props.model.name }}</code>。</div>
        <div v-else class="m26-alias-items-grid">
          <div v-for="item in props.aliases" :key="item.alias" class="m26-alias-item-card">
            <div class="m26-alias-card-top">
              <div class="flex items-center gap-2"><div class="m26-alias-name font-mono" :title="item.alias">{{ item.alias }}</div><span v-if="props.mappedAliases[item.alias] || props.mappedAliases[item.alias.toLowerCase()]" class="m26-custom-mapping-badge">手动归类</span></div>
              <div class="flex items-center gap-1">
                <button class="m26-alias-copy-btn" :title="'复制别名: ' + item.alias" @click.stop="emit('copy-alias', item.alias)">{{ props.copiedText === item.alias ? '已复制' : '复制' }}</button>
                <button v-if="props.mappedAliases[item.alias] || props.mappedAliases[item.alias.toLowerCase()]" class="m26-alias-unlink-btn" title="解除手动归类，恢复为独立模型" @click.stop="emit('remove-mapping', item.alias)">解除</button>
              </div>
            </div>
            <div class="m26-alias-providers-row"><span class="m26-alias-prov-label">对应供货渠道：</span><div class="m26-alias-prov-chips"><span v-for="channel in item.channels" :key="channel.name" class="m26-alias-prov-chip" :class="channel.status"><span class="prov-dot" :class="channel.status"></span><span>{{ channel.name }}</span><span v-if="latency(channel.latency)" class="ch-lat-num">{{ latency(channel.latency) }}</span></span></div></div>
          </div>
        </div>
      </div>

      <div class="m26-alias-section">
        <div class="m26-alias-section-head"><span class="m26-alias-section-title">各渠道调用与重写映射 ({{ props.channelDetails.length }})</span><span class="text-xs text-slate-400">HeiGate 路由到具体渠道时的实时模型转发与重写策略</span></div>
        <div class="m26-mapping-table-wrapper">
          <table class="m26-mapping-table">
            <thead><tr><th>渠道名称</th><th>实际转发上游模型</th><th>重写策略</th><th>状态与延迟</th></tr></thead><tbody>
            <tr v-for="channel in props.channelDetails" :key="channel.name">
              <td><div class="flex items-center gap-1.5 font-medium text-slate-800"><span class="prov-dot" :class="channel.status"></span><span>{{ channel.name }}</span></div></td>
              <td><div class="flex items-center gap-1.5 font-mono text-xs text-indigo-700 font-semibold"><span>{{ channel.upstreamModel }}</span><button class="m26-table-copy-icon" :title="'复制上游模型名: ' + channel.upstreamModel" @click.stop="emit('copy-alias', channel.upstreamModel)">复制</button></div></td>
              <td><span v-if="channel.upstreamModel !== props.model.name" class="m26-mapping-chip rewrite">↳ 别名重写</span><span v-else class="m26-mapping-chip direct">✓ 同名直通</span></td>
              <td><div class="flex items-center gap-1.5 text-xs text-slate-600"><span class="m26-status-text" :class="channel.status">{{ statusLabel(channel.status) }}</span><span v-if="latency(channel.latency)" class="font-mono text-slate-400">({{ latency(channel.latency) }})</span></div></td>
            </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>

    <div class="m26-modal-footer"><div class="m26-modal-tip text-xs text-slate-400"><span>HeiGate 会在分发调度时自动将统一模型名映射为上游别名，实现多渠道平滑无感负载均衡与故障熔断顺延。</span></div><button class="m26-action-btn" @click="emit('close')">关闭</button></div>
  </div>
</template>
