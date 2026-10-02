<script setup lang="ts">
import type { SystemInfo } from '@/services/gatewayApi'

defineProps<{
  systemInfo: SystemInfo | null
  fallbackLogsCount: number
  isPruningLogs: boolean
  logRetentionDays: number
  logMaxCount: number
}>()

const emit = defineEmits<{
  'prune-logs': []
  'copy-path': [path: string]
  'set-retention': [days: number]
  'set-max-count': [count: number]
  'export-config': []
  'import-config': []
  'clear-workspace': []
}>()

function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
}
</script>

<template>
  <div class="m26-pref-group">
    <div class="m26-pref-group-header">
      <div class="m26-pref-group-icon icon-storage">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
          <ellipse cx="12" cy="5" rx="9" ry="3" />
          <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
          <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
        </svg>
      </div>
      <div class="m26-pref-group-titles">
        <h3 class="m26-pref-title">数据存储与自动清理策略</h3>
        <span class="m26-pref-subtitle">管理本地 SQLite 数据库文件、历史请求流水保留周期及配置备份</span>
      </div>
    </div>

    <div class="m26-pref-card">
      <div class="m26-pref-row" v-if="systemInfo?.db_path">
        <div class="m26-pref-meta">
          <div class="flex items-center gap-2">
            <span class="label">SQLite 本地数据库</span>
            <span class="m26-db-chip">{{ formatBytes(systemInfo.db_size_bytes) }}</span>
            <span class="m26-db-chip">{{ (systemInfo.logs_count ?? fallbackLogsCount).toLocaleString() }} 条流水</span>
          </div>
          <span class="desc font-mono text-xs opacity-75 truncate max-w-lg" :title="systemInfo.db_path">
            {{ systemInfo.db_path }}
          </span>
        </div>
        <div class="m26-pref-ctrl">
          <button class="m26-glass-btn" :disabled="isPruningLogs" @click="emit('prune-logs')" title="按当前策略立即执行一次修剪清理">
            <span v-if="isPruningLogs" class="m26-spin-dot"></span>
            <svg v-else viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="23 4 23 10 17 10" />
              <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
            </svg>
            <span>{{ isPruningLogs ? '清理中...' : '立即执行清理' }}</span>
          </button>
          <button class="m26-glass-btn" @click="emit('copy-path', systemInfo.db_path)" title="复制数据库绝对路径">
            <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
            </svg>
            <span>复制路径</span>
          </button>
        </div>
      </div>

      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">自动清理保留时长</span>
          <span class="desc">后台每 30 分钟定时自动巡检，物理删除超过此保留天数的历史请求与 Token 流水</span>
        </div>
        <div class="m26-pref-ctrl">
          <select
            :value="logRetentionDays"
            class="m26-select"
            @change="emit('set-retention', Number(($event.target as HTMLSelectElement).value))"
          >
            <option :value="1">保留 1 天 (极轻量)</option>
            <option :value="3">保留 3 天 (日常轻量)</option>
            <option :value="7">保留 7 天 (默认推荐)</option>
            <option :value="14">保留 14 天</option>
            <option :value="30">保留 30 天</option>
            <option :value="0">永久保留 (不限时长)</option>
          </select>
        </div>
      </div>

      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">自动清理条数上限</span>
          <span class="desc">流水总记录数超出上限阈值时，后台静默按 FIFO（先进先出）修剪删除最早记录</span>
        </div>
        <div class="m26-pref-ctrl">
          <select
            :value="logMaxCount"
            class="m26-select"
            @change="emit('set-max-count', Number(($event.target as HTMLSelectElement).value))"
          >
            <option :value="1000">最多 1,000 条 (约 0.5 MB)</option>
            <option :value="3000">最多 3,000 条 (约 1.5 MB)</option>
            <option :value="5000">最多 5,000 条 (默认推荐 · 约 2.5 MB)</option>
            <option :value="10000">最多 10,000 条 (约 5 MB)</option>
            <option :value="50000">最多 50,000 条</option>
            <option :value="0">不限制条数</option>
          </select>
        </div>
      </div>

      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">配置备份与工作区重置</span>
          <span class="desc">以 JSON 格式完整导出本机通道与偏好设置供备份迁移，或一键重置当前工作区</span>
        </div>
        <div class="m26-pref-ctrl">
          <div class="flex items-center gap-2">
            <button class="m26-glass-btn" @click="emit('export-config')" title="导出当前所有渠道与配置">
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                <polyline points="7 10 12 15 17 10" />
                <line x1="12" y1="15" x2="12" y2="3" />
              </svg>
              <span>导出配置</span>
            </button>
            <button class="m26-glass-btn" @click="emit('import-config')" title="导入已有配置文件">
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                <polyline points="17 8 12 3 7 8" />
                <line x1="12" y1="3" x2="12" y2="15" />
              </svg>
              <span>导入配置</span>
            </button>
            <div class="m26-btn-separator"></div>
            <button class="m26-glass-btn danger" @click="emit('clear-workspace')" title="清空本机保存的所有通道配置">
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
              </svg>
              <span>清空工作区</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

