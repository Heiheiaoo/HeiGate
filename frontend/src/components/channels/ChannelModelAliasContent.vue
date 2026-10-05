<script setup lang="ts">
import type { AggregateModelAliasItem, AggregateModelChannelDetail, AggregateModelView } from './types'

type ModelFamily = {
  label: string
  color: string
  bg: string
  border: string
}

const props = withDefaults(
  defineProps<{
    model: AggregateModelView
    family?: ModelFamily
    aliases?: AggregateModelAliasItem[]
    channelDetails: AggregateModelChannelDetail[]
    search?: string
    copiedText?: string
    mappedAliases?: Record<string, string>
  }>(),
  {
    family: () => ({ label: '', color: '', bg: '', border: '' }),
    aliases: () => [],
    search: '',
    copiedText: '',
    mappedAliases: () => ({}),
  }
)

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

function isManualMapping(upstreamModel: string): boolean {
  return Boolean(
    props.mappedAliases[upstreamModel] ||
    props.mappedAliases[upstreamModel.toLowerCase()]
  )
}
</script>

<template>
  <div class="m26-modal-sheet m26-aliases-modal-sheet">
    <div class="m26-modal-header supplier-modal-header">
      <div class="flex items-center gap-2.5 min-w-0">
        <h3 class="m26-modal-title font-mono text-[15px] font-bold leading-none">{{ props.model.name }}</h3>
        <button class="m26-model-copy-icon-btn" :title="props.copiedText === props.model.name ? '已复制模型名' : '复制模型名: ' + props.model.name" @click.stop="emit('copy-model')">
          <svg v-if="props.copiedText !== props.model.name" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="5" y="5" width="8" height="8" rx="1.5" /><path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" /></svg>
          <svg v-else viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="#059669" stroke-width="2.2" aria-hidden="true"><polyline points="13.5 4.5 6.5 11.5 2.5 7.5" /></svg>
        </button>
        <span class="text-xs text-slate-400">共 {{ props.model.providerCount }} 个供货渠道</span>
      </div>
      <button class="m26-close-btn" aria-label="关闭" @click="emit('close')">✕</button>
    </div>

    <div class="m26-modal-body m26-alias-modal-body">
      <div class="m26-alias-section">
        <div class="m26-alias-section-head">
          <span class="m26-alias-section-title">各渠道调用与重写映射 ({{ props.channelDetails.length }})</span>
          <button
            class="m26-model-test-btn"
            title="在全部支持此模型的渠道中发起并发探活"
            @click.stop="emit('open-speed-test')"
          >
            <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
            </svg>
            <span>探活</span>
          </button>
        </div>
        <div class="m26-mapping-table-wrapper">
          <table class="m26-mapping-table">
            <thead>
              <tr>
                <th>渠道名称</th>
                <th>实际转发上游模型</th>
                <th>重写策略</th>
                <th>延迟</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="channel in props.channelDetails" :key="channel.name">
                <td>
                  <div class="flex items-center gap-1.5 font-medium text-slate-800">
                    <span class="prov-dot" :class="channel.status"></span>
                    <span>{{ channel.name }}</span>
                  </div>
                </td>
                <td>
                  <div class="font-mono text-xs text-indigo-700 font-semibold">
                    {{ channel.upstreamModel }}
                  </div>
                </td>
                <td>
                  <span v-if="channel.upstreamModel.toLowerCase() === props.model.name.toLowerCase()" class="m26-strategy-text direct">
                    ✓ 同名直通
                  </span>
                  <div v-else-if="isManualMapping(channel.upstreamModel)" class="flex items-center gap-1.5">
                    <span class="m26-strategy-text manual">↳ 手动归类</span>
                    <button class="m26-alias-unlink-btn" title="解除手动归类，恢复为独立模型" @click.stop="emit('remove-mapping', channel.upstreamModel)">解除</button>
                  </div>
                  <span v-else class="m26-strategy-text rewrite">
                    ↳ 别名重写
                  </span>
                </td>
                <td>
                  <span v-if="latency(channel.latency)" class="font-mono text-xs font-semibold text-slate-600">{{ latency(channel.latency) }}</span>
                  <span v-else-if="channel.status === 'offline'" class="text-xs text-rose-500 font-medium">离线</span>
                  <span v-else class="text-xs text-slate-400 font-mono">-</span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </div>
</template>
