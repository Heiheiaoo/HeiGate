<script setup lang="ts">
import { ref } from 'vue'
import type { ChannelFilterStatus, ChannelViewMode } from './types'

defineProps<{
  search: string
  filterStatus: ChannelFilterStatus
  viewMode: ChannelViewMode
  isProbingAll?: boolean
  totalCount: number
  healthyCount: number
  degradedCount: number
  errorCount: number
  disabledCount: number
}>()

const emit = defineEmits<{
  (e: 'update:search', val: string): void
  (e: 'update:filterStatus', val: ChannelFilterStatus): void
  (e: 'update:viewMode', val: ChannelViewMode): void
  (e: 'probe-all'): void
  (e: 'add-channel'): void
}>()

const searchInputRef = ref<HTMLInputElement | null>(null)

function focusSearch() {
  searchInputRef.value?.focus()
  searchInputRef.value?.select()
}

defineExpose({
  focusSearch,
})
</script>

<template>
  <div class="m26-ch-toolbar">
    <!-- Left: Search input, status filter pills, and view mode segmented button -->
    <div class="m26-ch-toolbar-left">
      <!-- Search Input -->
      <div class="m26-ch-search-box">
        <svg class="m26-ch-search-icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="6.5" cy="6.5" r="4.5" />
          <line x1="10" y1="10" x2="14" y2="14" />
        </svg>
        <input
          ref="searchInputRef"
          :value="search"
          type="text"
          class="m26-ch-search-input"
          placeholder="搜索渠道、端点或模型..."
          @input="emit('update:search', ($event.target as HTMLInputElement).value)"
        />
        <button
          v-if="search"
          class="m26-ch-search-clear"
          title="清空搜索 (Esc)"
          @click="emit('update:search', '')"
        >
          ✕
        </button>
        <span v-else class="m26-ch-kbd">⌘K</span>
      </div>

      <!-- Filter Status Pills -->
      <div class="m26-ch-filter-pills">
        <button
          class="m26-ch-filter-pill"
          :class="{ active: filterStatus === 'all' }"
          @click="emit('update:filterStatus', 'all')"
        >
          <span>全部</span>
          <span class="pill-count">{{ totalCount }}</span>
        </button>

        <button
          v-if="healthyCount > 0"
          class="m26-ch-filter-pill healthy"
          :class="{ active: filterStatus === 'healthy' }"
          @click="emit('update:filterStatus', 'healthy')"
        >
          <span class="pill-dot emerald"></span>
          <span>就绪</span>
          <span class="pill-count">{{ healthyCount }}</span>
        </button>

        <button
          v-if="degradedCount > 0"
          class="m26-ch-filter-pill degraded"
          :class="{ active: filterStatus === 'degraded' }"
          @click="emit('update:filterStatus', 'degraded')"
        >
          <span class="pill-dot amber"></span>
          <span>高延迟</span>
          <span class="pill-count">{{ degradedCount }}</span>
        </button>

        <button
          v-if="errorCount > 0"
          class="m26-ch-filter-pill offline"
          :class="{ active: filterStatus === 'offline' }"
          @click="emit('update:filterStatus', 'offline')"
        >
          <span class="pill-dot rose"></span>
          <span>异常</span>
          <span class="pill-count">{{ errorCount }}</span>
        </button>

        <button
          v-if="disabledCount > 0"
          class="m26-ch-filter-pill disabled"
          :class="{ active: filterStatus === 'disabled' }"
          @click="emit('update:filterStatus', 'disabled')"
        >
          <span class="pill-dot gray"></span>
          <span>停用</span>
          <span class="pill-count">{{ disabledCount }}</span>
        </button>
      </div>

      <!-- View Switcher (Grid vs List) -->
      <div class="m26-ch-segmented">
        <button
          class="m26-ch-seg-btn"
          :class="{ active: viewMode === 'grid' }"
          title="卡片网格视图"
          @click="emit('update:viewMode', 'grid')"
        >
          <svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
            <rect x="2" y="2" width="5" height="5" rx="1.2" />
            <rect x="9" y="2" width="5" height="5" rx="1.2" />
            <rect x="2" y="9" width="5" height="5" rx="1.2" />
            <rect x="9" y="9" width="5" height="5" rx="1.2" />
          </svg>
        </button>
        <button
          class="m26-ch-seg-btn"
          :class="{ active: viewMode === 'list' }"
          title="高密度列表视图"
          @click="emit('update:viewMode', 'list')"
        >
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="2" y1="4" x2="14" y2="4" />
            <line x1="2" y1="8" x2="14" y2="8" />
            <line x1="2" y1="12" x2="14" y2="12" />
          </svg>
        </button>
      </div>
    </div>

    <!-- Right: Actions (Batch Probe, Add Channel) -->
    <div class="m26-ch-toolbar-right">
      <button
        class="m26-ch-btn m26-ch-btn-secondary icon-only"
        :disabled="isProbingAll"
        :title="isProbingAll ? '探活中…' : '批量探活 (⌘⇧P)'"
        :aria-label="isProbingAll ? '批量探活进行中' : '批量探活全部渠道'"
        @click="emit('probe-all')"
      >
        <span v-if="isProbingAll" class="m26-spin-dot"></span>
        <svg v-else viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path d="M23 4v6h-6M1 20v-6h6" />
          <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
        </svg>
      </button>

      <button
        class="m26-ch-btn m26-ch-btn-primary icon-only"
        title="新建渠道 (⌘N)"
        aria-label="添加渠道"
        @click="emit('add-channel')"
      >
        <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true">
          <line x1="8" y1="3" x2="8" y2="13" />
          <line x1="3" y1="8" x2="13" y2="8" />
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.m26-ch-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 14px;
  flex-wrap: wrap;
}

.m26-ch-toolbar-left {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  flex: 1;
  min-width: 0;
}

.m26-ch-toolbar-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.m26-ch-search-box {
  position: relative;
  display: flex;
  align-items: center;
  width: 220px;
  background: rgba(255, 255, 255, 0.72);
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 8px;
  box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: all 0.15s ease;
}

.m26-ch-search-box:focus-within {
  border-color: #3b82f6;
  background: #ffffff;
  box-shadow: 0 0 0 2.5px rgba(59, 130, 246, 0.18);
  width: 260px;
}

.m26-ch-search-icon {
  margin-left: 8px;
  color: #94a3b8;
  flex-shrink: 0;
}

.m26-ch-search-input {
  width: 100%;
  height: 28px;
  padding: 0 6px;
  background: transparent;
  border: none;
  font-size: 12px;
  color: #1e293b;
  outline: none;
}

.m26-ch-search-input::placeholder {
  color: #94a3b8;
}

.m26-ch-search-clear {
  padding: 2px 6px;
  color: #94a3b8;
  font-size: 10px;
  cursor: pointer;
}

.m26-ch-search-clear:hover {
  color: #475569;
}

.m26-ch-kbd {
  margin-right: 6px;
  padding: 1px 4px;
  background: rgba(0, 0, 0, 0.05);
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 4px;
  font-size: 10px;
  font-family: inherit;
  color: #64748b;
  pointer-events: none;
}

.m26-ch-filter-pills {
  display: flex;
  align-items: center;
  gap: 4px;
  background: rgba(0, 0, 0, 0.03);
  padding: 2px;
  border-radius: 8px;
  border: 1px solid rgba(0, 0, 0, 0.04);
}

.m26-ch-filter-pill {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border-radius: 6px;
  font-size: 11px;
  color: #64748b;
  background: transparent;
  border: none;
  cursor: pointer;
  transition: all 0.15s ease;
  font-weight: 500;
}

.m26-ch-filter-pill:hover {
  color: #1e293b;
  background: rgba(255, 255, 255, 0.5);
}

.m26-ch-filter-pill.active {
  background: #ffffff;
  color: #0f172a;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.06);
  font-weight: 600;
}

.pill-count {
  font-size: 10px;
  opacity: 0.7;
  padding: 0 3px;
}

.pill-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}

.pill-dot.emerald { background: #10b981; }
.pill-dot.amber { background: #f59e0b; }
.pill-dot.rose { background: #ef4444; }
.pill-dot.gray { background: #94a3b8; }

.m26-ch-segmented {
  display: flex;
  align-items: center;
  background: rgba(0, 0, 0, 0.04);
  padding: 2px;
  border-radius: 7px;
  border: 1px solid rgba(0, 0, 0, 0.05);
}

.m26-ch-seg-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 24px;
  border-radius: 5px;
  background: transparent;
  border: none;
  color: #64748b;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-ch-seg-btn:hover {
  color: #1e293b;
}

.m26-ch-seg-btn.active {
  background: #ffffff;
  color: #0f172a;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.06);
}

/* Action Buttons */
.m26-ch-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 28px;
  padding: 0 12px;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
  transition: all 0.15s cubic-bezier(0.4, 0, 0.2, 1);
  box-sizing: border-box;
}

.m26-ch-btn.icon-only {
  width: 34px;
  height: 34px;
  padding: 0;
}

.m26-ch-btn-secondary {
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.09);
  color: #475569;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.m26-ch-btn-secondary:hover:not(:disabled) {
  background: #f8fafc;
  color: #0f172a;
  border-color: rgba(0, 0, 0, 0.16);
  box-shadow: 0 2px 5px rgba(0, 0, 0, 0.06);
  transform: translateY(-0.5px);
}

.m26-ch-btn-secondary:active:not(:disabled) {
  background: #f1f5f9;
  transform: translateY(0);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.m26-ch-btn-secondary:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.m26-ch-btn-primary {
  background: linear-gradient(135deg, #4f46e5 0%, #4338ca 100%);
  border: 1px solid rgba(255, 255, 255, 0.25);
  color: #ffffff;
  font-weight: 600;
  box-shadow: 0 2px 6px rgba(79, 70, 229, 0.28), 0 1px 2px rgba(0, 0, 0, 0.04);
}

.m26-ch-btn-primary:hover {
  filter: brightness(1.08);
  box-shadow: 0 4px 12px rgba(79, 70, 229, 0.38);
  transform: translateY(-1px);
}

.m26-ch-btn-primary:active {
  transform: translateY(0);
  box-shadow: 0 1px 3px rgba(79, 70, 229, 0.3);
}

/* Spin loader dot */
.m26-spin-dot {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 1.5px solid currentColor;
  border-top-color: transparent;
  border-radius: 50%;
  animation: m26Spin 0.7s linear infinite;
}

@keyframes m26Spin {
  to { transform: rotate(360deg); }
}
</style>
