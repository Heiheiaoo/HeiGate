<script setup lang="ts">
import type { Channel } from './types'
import { probeModeLabel, probeModeTooltip, isPeriodicProbe } from './types'

defineProps<{
  channels: Channel[]
  selectedChannel?: Channel | null
  searchQuery?: string
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
  (e: 'add-channel'): void
  (e: 'clear-search'): void
}>()

function formatLatencySec(ms: number | null | undefined): string {
  if (ms == null) return '超时'
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(2)}s`
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
    case 'clean': return '无注入'
    case 'suspected': return '疑似注入'
    case 'detected': return '发现注入'
    default: return '未检测'
  }
}
</script>

<template>
  <div v-if="channels.length > 0" class="m26-table-wrap">
    <table class="m26-channel-table">
      <thead>
        <tr>
          <th style="min-width: 160px;">渠道名称</th>
          <th style="min-width: 150px;">支持模型</th>
          <th style="width: 110px;">探活模式</th>
          <th style="width: 95px;">首字 TTFT</th>
          <th style="width: 110px;">健康状态</th>
          <th style="width: 90px;">防注入</th>
          <th style="width: 110px; text-align: right;">操作</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="c in channels"
          :key="c.gatewayId ?? c.name"
          class="m26-table-row"
          :class="{
            'is-selected': selectedChannel?.gatewayId ? selectedChannel.gatewayId === c.gatewayId : selectedChannel?.name === c.name,
            'is-disabled': !c.enabled
          }"
          @click="emit('select', c)"
        >
          <!-- 1. Channel Name -->
          <td>
            <div class="m26-cell-name-box">
              <span class="m26-cell-name" :title="`${c.name}\n协议: ${c.protocol}\n${c.url}`">{{ c.name }}</span>
            </div>
          </td>

          <!-- 2. Models -->
          <td>
            <div class="m26-cell-models" @click.stop="emit('open-models', c)" title="点击查看全部支持模型及探活">
              <span class="m26-model-tag">
                <span class="tag-bullet">⚡</span>
                <span class="tag-name truncate max-w-[130px] font-mono">{{ c.primaryModel || '默认模型' }}</span>
              </span>
              <span v-if="c.modelNames && c.modelNames.length > 1" class="m26-model-more-tag">
                +{{ c.modelNames.length - 1 }}
              </span>
            </div>
          </td>

          <!-- 3. Probe Mode -->
          <td>
            <button
              class="m26-probe-mode-tag"
              :class="{ periodic: isPeriodicProbe(c) }"
              :title="probeModeTooltip(c)"
              @click.stop="emit('edit', c)"
            >
              <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.8" class="shrink-0">
                <path d="M2.5 4h11M2.5 8h7M2.5 12h5" stroke-linecap="round" />
              </svg>
              <span>{{ probeModeLabel(c) }}</span>
            </button>
          </td>

          <!-- 5. Latency / TTFT -->
          <td>
            <span
              class="font-mono text-xs"
              :class="{
                'text-emerald-600 font-semibold': c.status === 'healthy',
                'text-amber-600': c.status === 'degraded',
                'text-slate-400': c.status !== 'healthy' && c.status !== 'degraded'
              }"
              :title="`探活主模型 [${c.primaryModel || '默认'}] 首字响应 (TTFT): ${formatLatencySec(c.ttft ?? c.latency)}`"
            >
              {{ formatLatencySec(c.ttft ?? c.latency) }}
            </span>
          </td>

          <!-- 6. Status & Diagnostic -->
          <td>
            <div
              class="m26-status-cell"
              :title="c.lastError ? `探活主模型 [${c.primaryModel || '默认'}] 状态: ${statusText(c.status)} (${c.lastError})` : `探活主模型 [${c.primaryModel || '默认'}] 状态: ${statusText(c.status)}`"
            >
              <span class="m26-status-dot" :class="c.status"></span>
              <span class="m26-status-label">{{ statusText(c.status) }}</span>
              <button
                v-if="c.status !== 'healthy' && c.lastError"
                class="m26-err-link-tag"
                :title="`主模型调用异常，点击查看报错排查建议: ${c.lastError}`"
                @click.stop="emit('open-diagnostic', c)"
              >
                排障
              </button>
            </div>
          </td>

          <!-- 7. Prompt Injection -->
          <td>
            <button
              class="m26-inject-tag"
              :class="injectionClass(c.injectionState)"
              :title="`提示词注入检测: ${injectionLabel(c.injectionState)} (点击重新检测)`"
              @click.stop="emit('open-inject', c)"
            >
              {{ injectionLabel(c.injectionState) }}
            </button>
          </td>

          <!-- Actions: Enable switch, Edit, Delete -->
          <td class="text-right" @click.stop>
            <div class="m26-table-actions">
              <!-- Enable / Disable Switch -->
              <label class="m26-table-switch" :title="c.enabled ? '已启用（点击停用）' : '已停用（点击启用）'">
                <input
                  type="checkbox"
                  :checked="c.enabled"
                  class="m26-table-switch-input"
                  @change="emit('toggle', c)"
                />
                <span class="m26-table-switch-slider"></span>
              </label>

              <!-- Edit -->
              <button
                class="m26-tbl-icon-btn"
                title="编辑渠道配置"
                @click="emit('edit', c)"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                  <path d="M11 2l3 3-8 8H3v-3l8-8z" />
                </svg>
              </button>

              <!-- Delete -->
              <button
                class="m26-tbl-icon-btn danger"
                title="删除渠道"
                @click="emit('remove', c)"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                  <polyline points="3 4 5 4 13 4" />
                  <path d="M5 4v8a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V4" />
                </svg>
              </button>
            </div>
          </td>
        </tr>
      </tbody>
    </table>
  </div>

  <!-- Empty State -->
  <div v-else class="m26-ch-empty-card">
    <div class="empty-icon-box">
      <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" stroke-width="1.6">
        <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
        <line x1="8" y1="21" x2="16" y2="21" />
        <line x1="12" y1="17" x2="12" y2="21" />
      </svg>
    </div>
    <div class="empty-text-box">
      <h5 v-if="searchQuery" class="empty-title">未找到匹配 “{{ searchQuery }}” 的渠道</h5>
      <h5 v-else class="empty-title">暂无已配置的供应商渠道</h5>
      <p class="empty-subtitle">
        {{ searchQuery ? '请检查搜索关键词或重置筛选条件' : '添加官方 API、中转商或本地 Ollama 渠道，开启统一本地网关' }}
      </p>
    </div>
    <div class="empty-actions">
      <button
        v-if="searchQuery"
        class="m26-glass-btn text-xs"
        @click="emit('clear-search')"
      >
        清空筛选
      </button>
      <button
        v-else
        class="m26-action-btn primary text-xs"
        @click="emit('add-channel')"
      >
        添加第一个渠道
      </button>
    </div>
  </div>
</template>

<style scoped>
.m26-table-wrap {
  width: 100%;
  overflow-x: auto;
  background: rgba(255, 255, 255, 0.85);
  backdrop-filter: blur(12px);
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 12px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.m26-channel-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  text-align: left;
}

.m26-channel-table thead tr {
  background: rgba(0, 0, 0, 0.02);
  border-bottom: 1px solid rgba(0, 0, 0, 0.06);
}

.m26-channel-table th {
  padding: 8px 12px;
  font-size: 11px;
  font-weight: 600;
  color: #64748b;
  white-space: nowrap;
}

.m26-table-row {
  border-bottom: 1px solid rgba(0, 0, 0, 0.04);
  transition: background 0.12s ease;
  cursor: pointer;
}

.m26-table-row:hover {
  background: rgba(59, 130, 246, 0.035);
}

.m26-table-row.is-selected {
  background: rgba(59, 130, 246, 0.08);
}

.m26-table-row.is-disabled {
  opacity: 0.55;
  filter: grayscale(40%);
}

.m26-channel-table td {
  padding: 8px 12px;
  vertical-align: middle;
}

/* Switch in Table */
.m26-table-switch {
  position: relative;
  display: inline-block;
  width: 24px;
  height: 14px;
  cursor: pointer;
  flex-shrink: 0;
  margin-right: 6px;
}

.m26-table-switch-input {
  opacity: 0;
  width: 0;
  height: 0;
}

.m26-table-switch-slider {
  position: absolute;
  inset: 0;
  background-color: #cbd5e1;
  border-radius: 14px;
  transition: 0.15s;
}

.m26-table-switch-slider::before {
  position: absolute;
  content: "";
  height: 10px;
  width: 10px;
  left: 2px;
  bottom: 2px;
  background-color: white;
  border-radius: 50%;
  transition: 0.15s;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.15);
}

.m26-table-switch-input:checked + .m26-table-switch-slider {
  background-color: #10b981;
}

.m26-table-switch-input:checked + .m26-table-switch-slider::before {
  transform: translateX(10px);
}

/* Name & URL Cell */
.m26-cell-name-box {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.m26-cell-name {
  font-weight: 600;
  color: #0f172a;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}


/* Models Cell */
.m26-cell-models {
  display: flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
}

.m26-model-tag {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  background: rgba(0, 0, 0, 0.035);
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 11px;
  color: #334155;
  border: 1px solid rgba(0, 0, 0, 0.04);
}

.m26-model-tag:hover {
  background: rgba(59, 130, 246, 0.1);
  color: #2563eb;
  border-color: rgba(59, 130, 246, 0.2);
}

.tag-bullet {
  color: #f59e0b;
  font-size: 10px;
}

.m26-model-more-tag {
  font-size: 10px;
  padding: 1px 4px;
  border-radius: 3px;
  background: rgba(0, 0, 0, 0.05);
  color: #64748b;
}

/* Probe Mode Tag */
.m26-probe-mode-tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  font-weight: 500;
  padding: 2px 7px;
  border-radius: 5px;
  background: #f1f5f9;
  color: #475569;
  border: 1px solid #e2e8f0;
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
  transition: all 0.12s ease;
}

.m26-probe-mode-tag:hover {
  background: #e2e8f0;
  color: #1e293b;
}

.m26-probe-mode-tag.periodic {
  background: rgba(99, 102, 241, 0.08);
  color: #4f46e5;
  border-color: rgba(99, 102, 241, 0.2);
}

.m26-probe-mode-tag.periodic:hover {
  background: rgba(99, 102, 241, 0.15);
}

/* Status Cell */
.m26-status-cell {
  display: flex;
  align-items: center;
  gap: 5px;
}

.m26-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}
.m26-status-dot.healthy { background: #10b981; box-shadow: 0 0 4px #10b981; }
.m26-status-dot.degraded { background: #f59e0b; }
.m26-status-dot.offline { background: #ef4444; }
.m26-status-dot.unknown { background: #94a3b8; }

.m26-status-label {
  font-size: 11px;
  color: #475569;
}

.m26-err-link-tag {
  font-size: 9.5px;
  padding: 0 4px;
  border-radius: 3px;
  background: #fee2e2;
  color: #dc2626;
  border: 1px solid rgba(220, 38, 38, 0.2);
  cursor: pointer;
}
.m26-err-link-tag:hover {
  background: #fecaca;
}

/* Injection Tag */
.m26-inject-tag {
  font-size: 10.5px;
  padding: 1.5px 6px;
  border-radius: 4px;
  border: 1px solid transparent;
  cursor: pointer;
}

.inject-clean { background: rgba(16, 185, 129, 0.08); color: #059669; border-color: rgba(16, 185, 129, 0.15); }
.inject-suspected { background: rgba(245, 158, 11, 0.1); color: #d97706; border-color: rgba(245, 158, 11, 0.2); }
.inject-detected { background: rgba(239, 68, 68, 0.12); color: #dc2626; border-color: rgba(239, 68, 68, 0.2); font-weight: 600; }
.inject-unknown { background: rgba(0, 0, 0, 0.03); color: #94a3b8; }

/* Table Actions */
.m26-table-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 3px;
}

.m26-tbl-icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: 4px;
  background: transparent;
  border: none;
  color: #64748b;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-tbl-icon-btn:hover:not(:disabled) {
  background: rgba(0, 0, 0, 0.05);
  color: #0f172a;
}

.m26-tbl-icon-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.m26-tbl-icon-btn.danger:hover:not(:disabled) {
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
  display: inline-block;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.m26-ch-empty-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px 24px;
  background: rgba(255, 255, 255, 0.6);
  border: 1px dashed rgba(0, 0, 0, 0.12);
  border-radius: 12px;
  text-align: center;
}

.empty-icon-box {
  color: #94a3b8;
  margin-bottom: 12px;
}

.empty-title {
  margin: 0 0 4px;
  font-size: 14px;
  font-weight: 600;
  color: #1e293b;
}

.empty-subtitle {
  margin: 0 0 16px;
  font-size: 12px;
  color: #64748b;
  max-width: 320px;
}
</style>
