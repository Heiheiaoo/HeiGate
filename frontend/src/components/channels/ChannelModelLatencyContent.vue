<script setup lang="ts">
import type { Channel, ModelLatencyRow } from './types'

const props = defineProps<{
  channel: Channel
  rows: ModelLatencyRow[]
  selectedCount: number
  allSelected: boolean
  someSelected: boolean
  probingAll: boolean
  checkingAllInjections: boolean
}>()

const emit = defineEmits<{
  (e: 'select-all'): void
  (e: 'delete-selected'): void
  (e: 'delete-problematic'): void
  (e: 'batch-action', action: 'probe' | 'injection'): void
  (e: 'toggle-selection', modelName: string): void
  (e: 'card-click', modelName: string): void
  (e: 'copy-model', modelName: string): void
  (e: 'open-model-detail', modelName: string): void
  (e: 'open-injection-detail', modelName: string): void
  (e: 'probe-model', modelName: string): void
  (e: 'check-injection', modelName: string): void
}>()
</script>

<template>
  <div class="m26-modal-toolbar">
    <div class="flex items-center gap-3">
      <label class="m26-select-all-label">
        <input
          type="checkbox"
          :checked="props.allSelected"
          :indeterminate="props.someSelected"
          @change="emit('select-all')"
        />
        <span>全选</span>
      </label>

      <button
        v-if="props.selectedCount > 0"
        class="m26-action-btn danger-subtle"
        :title="`删除已选 ${props.selectedCount} 个模型`"
        :aria-label="`删除已选 ${props.selectedCount} 个模型`"
        @click="emit('delete-selected')"
      >
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
          <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6" />
        </svg>
        <span>{{ props.selectedCount }}</span>
      </button>
      <button
        class="m26-action-btn danger-subtle m26-delete-problematic"
        :disabled="!props.rows.some(row => row.failed) || props.probingAll || props.checkingAllInjections || props.rows.some(row => row.probing || row.checkingInjection)"
        title="删除已探活失败或超时的模型；未检测模型会保留"
        :aria-label="`删除问题模型 ${props.rows.filter(row => row.failed).length} 个`"
        @click="emit('delete-problematic')"
      >
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
          <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6" />
        </svg>
        <span>删除问题模型</span>
        <span>{{ props.rows.filter(row => row.failed).length }}</span>
      </button>
    </div>

    <div class="m26-model-batch-actions">
      <span v-if="props.selectedCount" class="m26-model-batch-count">
        已选 {{ props.selectedCount }}/{{ props.channel.modelNames?.length || 0 }}
      </span>
      <button
        class="m26-model-batch-btn probe"
        :class="{ 'is-loading': props.probingAll }"
        :disabled="props.probingAll || props.checkingAllInjections"
        :title="`${props.probingAll ? '探活中' : '探活'}${props.selectedCount ? `已选 ${props.selectedCount} 个` : `全部 ${props.channel.modelNames?.length || 0} 个`}模型`"
        :aria-label="`${props.probingAll ? '探活中' : '批量探活'}${props.selectedCount ? `已选 ${props.selectedCount} 个` : `全部 ${props.channel.modelNames?.length || 0} 个`}模型`"
        @click="emit('batch-action', 'probe')"
      >
        <span v-if="props.probingAll" class="m26-spin-dot"></span>
        <svg v-else viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
          <path d="M2 10h3l2-5 4 10 2-5h5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
      <button
        class="m26-model-batch-btn injection"
        :disabled="props.probingAll || props.checkingAllInjections"
        :title="`${props.checkingAllInjections ? '检测中' : '检测输入统计'}${props.selectedCount ? `已选 ${props.selectedCount} 个` : `全部 ${props.channel.modelNames?.length || 0} 个`}模型`"
        :aria-label="`${props.checkingAllInjections ? '检测中' : '批量检测输入统计'}${props.selectedCount ? `已选 ${props.selectedCount} 个` : `全部 ${props.channel.modelNames?.length || 0} 个`}模型`"
        @click="emit('batch-action', 'injection')"
      >
        <span v-if="props.checkingAllInjections" class="m26-spin-dot"></span>
        <svg v-else viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
          <path d="M10 2 16 4.3v4.6c0 4.1-2.5 6.8-6 9.1-3.5-2.3-6-5-6-9.1V4.3L10 2Z" />
          <path d="m7.2 9.7 1.8 1.8 3.8-4" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
    </div>
  </div>

  <div class="m26-modal-body m26-model-list-body">
    <div class="m26-model-grid">
      <div
        v-for="(row, idx) in props.rows"
        :key="row.name"
        class="m26-model-item-card"
        :class="{ 'is-selected': row.selected }"
        @click="emit('card-click', row.name)"
      >
        <div class="flex items-center gap-2.5 min-w-0 flex-1">
          <label class="m26-model-checkbox" @click.stop>
            <input
              type="checkbox"
              :checked="row.selected"
              @change="emit('toggle-selection', row.name)"
            />
          </label>

          <div class="m26-model-item-info">
            <div class="flex items-center gap-2">
              <span class="m26-model-idx">#{{ idx + 1 }}</span>
              <span class="m26-model-full-name" :title="row.name">{{ row.name }}</span>
              <button
                class="m26-model-copy-btn m26-model-row-copy"
                :title="row.copied ? 'Model name copied' : 'Copy model name'"
                :aria-label="row.copied ? `Model name copied: ${row.name}` : `Copy model name: ${row.name}`"
                @click.stop="emit('copy-model', row.name)"
              >
                <svg v-if="!row.copied" viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                  <rect x="5" y="5" width="8" height="8" rx="1.5" />
                  <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                </svg>
                <svg v-else viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="#059669" stroke-width="2.2" aria-hidden="true">
                  <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                </svg>
              </button>
            </div>
            <span v-if="row.checkedAtLabel" class="m26-model-checked-at">最近检测 {{ row.checkedAtLabel }}</span>
          </div>
        </div>

        <div class="m26-model-item-actions" @click.stop>
          <div
            class="m26-model-lat-pill"
            :class="[row.latencyClass, { 'has-err-link': row.failed }]"
            :title="row.latencyTooltip"
            @click="emit('open-model-detail', row.name)"
          >
            <span class="lat-dot"></span>
            <span class="lat-text">{{ row.latencyDisplay }}</span>
            <svg v-if="row.failed" class="lat-view-err" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
              <circle cx="8" cy="8" r="6" />
              <path d="M8 4.5v4M8 11.3v.2" />
            </svg>
          </div>

          <div
            class="m26-model-inject-pill"
            :class="row.injectionClass"
            :title="row.injectionTooltip"
            @click="emit('open-injection-detail', row.name)"
          >
            <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2.2" class="inject-shield" aria-hidden="true">
              <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
            </svg>
            <span class="inject-text">{{ row.injectionLabel }}</span>
          </div>

          <button
            class="m26-btn-micro probe"
            :class="{ 'is-loading': row.probing }"
            :disabled="row.probing || props.probingAll"
            :title="row.probing ? '探活中…' : '探活模型'"
            :aria-label="row.probing ? '探活中' : `探活 ${row.name}`"
            @click="emit('probe-model', row.name)"
          >
            <span v-if="row.probing" class="m26-spin-dot"></span>
            <svg v-else viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
              <path d="M2 10h3l2-5 4 10 2-5h5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>

          <button
            class="m26-btn-micro inject"
            :disabled="row.checkingInjection || props.checkingAllInjections"
            :title="row.checkingInjection ? '检测中…' : '检测输入统计'"
            :aria-label="row.checkingInjection ? '检测中' : `检测 ${row.name} 的输入统计`"
            @click="emit('check-injection', row.name)"
          >
            <span v-if="row.checkingInjection" class="m26-spin-dot"></span>
            <svg v-else viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
              <path d="M10 2 16 4.3v4.6c0 4.1-2.5 6.8-6 9.1-3.5-2.3-6-5-6-9.1V4.3L10 2Z" />
              <path d="m7.2 9.7 1.8 1.8 3.8-4" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
