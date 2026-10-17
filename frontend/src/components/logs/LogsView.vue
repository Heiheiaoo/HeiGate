<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'
import type { RequestLog } from '@/services/gatewayApi'

const props = defineProps<{
  logs: RequestLog[]
  totalLogs: number
  search: string
  streamConnected: boolean
  autoScroll: boolean
  clearing: boolean
}>()

const emit = defineEmits<{
  (e: 'update:search', value: string): void
  (e: 'update:auto-scroll', value: boolean): void
  (e: 'clear'): void
  (e: 'open-detail', log: RequestLog): void
}>()

const tableWrapRef = ref<HTMLDivElement | null>(null)

function scrollToTop() {
  if (tableWrapRef.value) {
    tableWrapRef.value.scrollTo({ top: 0, behavior: 'smooth' })
  }
}

watch(
  () => props.logs.length,
  () => {
    if (props.autoScroll) {
      nextTick(() => {
        scrollToTop()
      })
    }
  }
)

defineExpose({
  scrollToTop,
  tableWrapRef,
})

function formatDate(value: string): string {
  try { return new Date(value).toLocaleString('zh-CN', { hour12: false }) } catch { return value }
}
function duration(value?: number): string {
  if (!value || value <= 0) return '-'
  return value < 1000 ? `${value}ms` : `${(value / 1000).toFixed(2)}s`
}
</script>

<template>
  <section class="m26-section-view m26-logs-section">
    <div class="m26-logs-header">
      <div class="m26-logs-left">
        <div class="m26-logs-search-box">
          <svg class="m26-logs-search-icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="6.5" cy="6.5" r="4.5" />
            <line x1="10" y1="10" x2="14" y2="14" />
          </svg>
          <input
            :value="props.search"
            type="text"
            class="m26-logs-search-input"
            placeholder="搜索流水日志..."
            @input="emit('update:search', ($event.target as HTMLInputElement).value)"
          />
          <button
            v-if="props.search"
            class="m26-logs-search-clear"
            title="清空搜索"
            @click="emit('update:search', '')"
          >
            ✕
          </button>
        </div>
      </div>

      <div class="m26-logs-right">
        <div
          class="m26-stream-status-pill"
          :class="{ connected: props.streamConnected }"
          :title="props.streamConnected ? '本地实时事件推流已就绪' : '本地实时流连接中...'"
        >
          <span class="m26-live-indicator"></span>
          <span>{{ props.streamConnected ? '实时推流' : '连接中' }}</span>
        </div>

        <button
          class="m26-glass-btn m26-logs-btn"
          :class="{ 'm26-btn-active-blue active': props.autoScroll }"
          :title="props.autoScroll ? '点击暂停自动滚动' : '点击开启新日志自动滚动到顶'"
          @click="emit('update:auto-scroll', !props.autoScroll)"
        >
          <span>{{ props.autoScroll ? '📜 实时滚动：开' : '⏸ 实时滚动：关' }}</span>
        </button>

        <button
          class="m26-glass-btn m26-logs-btn danger"
          :disabled="props.clearing"
          @click="emit('clear')"
        >
          清空流水
        </button>
      </div>
    </div>

    <div v-if="props.logs.length" class="m26-logs-table-wrap" ref="tableWrapRef">
      <table class="m26-table font-mono text-xs">
        <thead>
          <tr>
            <th style="width: 25%; min-width: 170px;" class="whitespace-nowrap">模型</th>
            <th style="width: 20%; min-width: 165px;" class="whitespace-nowrap">时间</th>
            <th style="width: 17%; min-width: 130px;" class="whitespace-nowrap">用时 / 首字</th>
            <th style="width: 10%; min-width: 70px;" class="whitespace-nowrap">提示</th>
            <th style="width: 10%; min-width: 70px;" class="whitespace-nowrap">补全</th>
            <th style="width: 10%; min-width: 85px;" class="whitespace-nowrap">状态</th>
            <th style="width: 8%; min-width: 65px;" class="text-right whitespace-nowrap">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="log in props.logs"
            :key="log.id"
            class="cursor-pointer transition-colors"
            :class="[log.status_code === 0 ? 'm26-row-in-flight' : 'hover:bg-slate-50/80']"
            title="点击查看请求与轮转详情"
            @click="emit('open-detail', log)"
          >
            <!-- 1. 模型 -->
            <td>
              <div class="flex flex-col items-start w-fit max-w-full">
                <span class="m26-model-badge" :title="log.model">
                  <span class="truncate max-w-[160px] font-mono">{{ log.model }}</span>
                </span>
                <span
                  v-if="log.upstream_model && log.upstream_model !== log.model"
                  class="text-[10px] text-slate-400 font-mono pl-1 pt-0.5 truncate max-w-[160px]"
                  :title="'映射至上游: ' + log.upstream_model"
                >
                  ➔ {{ log.upstream_model }}
                </span>
              </div>
            </td>

            <!-- 2. 时间 -->
            <td class="text-muted whitespace-nowrap m26-td-time">{{ formatDate(log.created_at) }}</td>

            <!-- 3. 用时 / 首字 -->
            <td class="whitespace-nowrap">
              <div class="flex items-center gap-1.5 flex-nowrap">
                <span class="m26-pill-total">{{ duration(log.latency_ms) }}</span>
                <span class="m26-pill-ttft">{{ duration(log.ttft_ms) }}</span>
                <span v-if="log.is_stream" class="m26-pill-stream">流</span>
              </div>
            </td>

            <!-- 4. 提示 -->
            <td class="text-slate-600 font-mono text-xs whitespace-nowrap">{{ log.prompt_tokens ?? 0 }}</td>

            <!-- 5. 补全 -->
            <td class="whitespace-nowrap">
              <div class="flex items-center gap-1 font-mono text-xs text-slate-800 font-medium">
                <span v-if="log.status_code === 0" class="m26-token-pulse-dot"></span>
                <span>{{ log.completion_tokens ?? 0 }}</span>
              </div>
            </td>

            <!-- 6. 状态 -->
            <td class="whitespace-nowrap m26-td-status">
              <span
                class="m26-status-pill"
                :class="log.status_code === 0 ? 'status-streaming' : log.is_failover && log.status_code >= 200 && log.status_code < 300 ? 'status-failover' : log.status_code >= 200 && log.status_code < 300 ? 'status-200' : log.status_code === 429 ? 'status-429' : 'status-err'"
              >
                {{ log.status_code === 0 ? '传输中' : log.is_failover && log.status_code >= 200 && log.status_code < 300 ? '轮转 200' : log.status_code >= 200 && log.status_code < 300 ? '200 OK' : log.status_code === 429 ? '429 限流' : (log.status_code + ' 异常') }}
              </span>
            </td>

            <!-- 7. 操作按钮 -->
            <td class="text-right whitespace-nowrap m26-td-action">
              <span class="m26-link-btn text-xs">详情</span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- Bottom count bar -->
    <div v-if="props.logs.length" class="m26-logs-footer">
      <span class="m26-logs-footer-text font-mono">
        共 {{ props.totalLogs }} 条流水<template v-if="props.search && props.logs.length !== props.totalLogs">（已过滤显示 {{ props.logs.length }} 条）</template>
      </span>
    </div>

    <div v-else class="m26-empty-state">
      <p v-if="props.search">没有匹配“{{ props.search }}”的请求流水。</p>
      <p v-else>暂无网关请求流水。当第三方客户端向本地网关发起请求时将实时记录。</p>
    </div>
  </section>
</template>

<style scoped>
.m26-logs-section {
  flex: 1;
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  overflow: hidden;
  max-width: 1400px;
  width: 100%;
  margin: 0 auto;
}

.m26-logs-header {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 0;
}

.m26-logs-left {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 1;
  min-width: 0;
}

.m26-logs-search-box {
  position: relative;
  display: flex;
  align-items: center;
  width: 220px;
  height: 28px;
  background: rgba(255, 255, 255, 0.72);
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 8px;
  box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: all 0.15s ease;
  box-sizing: border-box;
}

.m26-logs-search-box:focus-within {
  border-color: #3b82f6;
  background: #ffffff;
  box-shadow: 0 0 0 2.5px rgba(59, 130, 246, 0.18);
  width: 260px;
}

.m26-logs-search-icon {
  margin-left: 8px;
  color: #94a3b8;
  flex-shrink: 0;
}

.m26-logs-search-input {
  width: 100%;
  height: 100%;
  padding: 0 6px 0 6px;
  background: transparent;
  border: none;
  font-size: 11.5px;
  color: #1e293b;
  outline: none;
}

.m26-logs-search-input::placeholder {
  color: #94a3b8;
}

.m26-logs-search-clear {
  padding: 2px 6px;
  color: #94a3b8;
  font-size: 10px;
  background: transparent;
  border: none;
  cursor: pointer;
}

.m26-logs-search-clear:hover {
  color: #475569;
}

.m26-logs-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.m26-stream-status-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 28px;
  padding: 0 9px;
  font-size: 11px;
  font-weight: 550;
  border-radius: 8px;
  background: rgba(16, 185, 129, 0.08);
  border: 1px solid rgba(16, 185, 129, 0.25);
  color: #059669;
  box-sizing: border-box;
  user-select: none;
}

.m26-stream-status-pill:not(.connected) {
  background: rgba(245, 158, 11, 0.08);
  border-color: rgba(245, 158, 11, 0.25);
  color: #d97706;
}

.m26-logs-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 28px;
  padding: 0 10px;
  border-radius: 8px;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.08);
  color: #475569;
  font-size: 11px;
  font-weight: 500;
  cursor: pointer;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: all 0.15s ease;
  white-space: nowrap;
  user-select: none;
  box-sizing: border-box;
}

.m26-logs-btn:hover {
  background: #f8fafc;
  color: #1e293b;
  border-color: rgba(0, 0, 0, 0.15);
}

.m26-logs-btn.active,
.m26-logs-btn.m26-btn-active-blue {
  background: rgba(59, 130, 246, 0.08) !important;
  color: #2563eb !important;
  border-color: rgba(59, 130, 246, 0.3) !important;
  font-weight: 600;
}

.m26-logs-btn.danger {
  color: #e11d48;
  border-color: rgba(225, 29, 72, 0.2);
  background: #ffffff;
}

.m26-logs-btn.danger:hover {
  background: rgba(225, 29, 72, 0.06);
  border-color: #e11d48;
}

.m26-logs-table-wrap {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: auto;
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  border: 1px solid var(--m26-border);
  border-radius: 12px;
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.02);
  margin-bottom: 0;
  scrollbar-width: none !important;
  -ms-overflow-style: none !important;
}

.m26-logs-table-wrap::-webkit-scrollbar {
  display: none !important;
  width: 0 !important;
  height: 0 !important;
}

.m26-logs-table-wrap .m26-table {
  width: 100%;
  border-collapse: collapse;
  text-align: left;
  table-layout: fixed;
}

.m26-logs-table-wrap .m26-table thead th {
  position: sticky;
  top: 0;
  z-index: 5;
  background: #f8fafc;
  padding: 8px 10px;
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-muted);
  border-bottom: 1px solid var(--m26-border);
  white-space: nowrap;
}

.m26-logs-table-wrap .m26-table td {
  padding: 8px 10px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.04);
  color: var(--m26-text-secondary);
  vertical-align: middle;
  overflow: hidden;
  text-overflow: ellipsis;
}

.m26-logs-table-wrap .m26-table td.m26-td-time,
.m26-logs-table-wrap .m26-table td.m26-td-status,
.m26-logs-table-wrap .m26-table td.m26-td-action {
  overflow: visible !important;
  text-overflow: clip !important;
  white-space: nowrap !important;
}

.m26-logs-table-wrap .m26-table td.m26-td-action .m26-link-btn {
  white-space: nowrap !important;
  display: inline-block;
}

.m26-logs-footer {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  padding: 2px 4px 10px;
}

.m26-logs-footer-text {
  font-size: 11px;
  color: #94a3b8;
  font-weight: 500;
  user-select: none;
}
</style>
