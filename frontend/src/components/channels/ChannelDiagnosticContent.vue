<script setup lang="ts">
import type { Channel } from './types'

const props = defineProps<{
  channel: Channel
}>()

const emit = defineEmits<{
  (e: 'copy-raw', value: string): void
}>()

type TroubleshootingAdvice = {
  title: string
  detail: string
}

function formatErrorSummary(rawError?: string): string {
  if (!rawError) return '未知异常'
  if (rawError.includes('404')) {
    if (rawError.includes('model route not found') || rawError.includes('NOT_FOUND')) return 'HTTP 404: 上游模型路由不存在 (NOT_FOUND)'
    return 'HTTP 404: 目标端点或模型不存在 (Not Found)'
  }
  if (rawError.includes('401')) return 'HTTP 401: API Key 鉴权失败，客户端未授权 (UNAUTHENTICATED)'
  if (rawError.includes('429')) return 'HTTP 429: 上游超频限流或账户额度耗尽 (Rate limit / Quota exceeded)'
  if (rawError.includes('timeout') || rawError.includes('deadline')) return '网络请求超时: 上游未在规定时间内返回响应'
  if (rawError.includes('connection refused')) return '连接被拒绝: 目标服务器端口未开放或防火墙阻拦'
  return rawError.length > 90 ? `${rawError.slice(0, 90)}...` : rawError
}

function extractErrorCode(rawError?: string): string {
  if (!rawError) return 'ERROR'
  if (rawError.includes('404')) return 'HTTP 404'
  if (rawError.includes('401')) return 'HTTP 401'
  if (rawError.includes('429')) return 'HTTP 429'
  if (rawError.includes('500')) return 'HTTP 500'
  if (rawError.includes('502')) return 'HTTP 502'
  if (rawError.includes('504')) return 'HTTP 504'
  if (rawError.includes('timeout')) return 'TIMEOUT'
  return 'FAIL'
}

function getTroubleshootingAdvice(channel: Channel): TroubleshootingAdvice[] {
  const error = channel.lastError || ''
  const tips: TroubleshootingAdvice[] = []
  if (error.includes('404')) {
    tips.push({ title: '核对探活主模型', detail: `上游可能未开通当前主模型（${channel.primaryModel || channel.modelNames?.[0] || '未知'}），或者官方已经迭代该版本。` })
    tips.push({ title: '检查 Base URL 格式', detail: `当前为 ${channel.url}。请确认该地址是否缺少或多加了版本路径（例如 /v1）。` })
  } else if (error.includes('401')) {
    tips.push({ title: '检查 API 密钥', detail: '上游返回未授权。请确认 API Key 是否正确、前后有无多余空格，以及该 Key 是否仍然有效。' })
    tips.push({ title: '检查 Key 权限与分组', detail: '部分 OneAPI、NewAPI 或中转服务要求令牌绑定对应模型或分组后才能调用。' })
  } else if (error.includes('429')) {
    tips.push({ title: '检查账号额度', detail: '上游返回并发或配额超限，请登录服务商控制台查看余额和频率限制。' })
    tips.push({ title: '调大探活周期', detail: '将探活设置为仅手动或更长周期，避免探活本身占用频控配额。' })
  } else if (error.includes('timeout') || error.includes('deadline')) {
    tips.push({ title: '检查本地网络与代理', detail: `当前计算机连接 ${channel.url} 超时，请确认网络和代理配置。` })
  } else {
    tips.push({ title: '查看原始报错信息', detail: '参考下方上游完整返回内容，排查服务商给出的具体错误原因。' })
  }
  tips.push({ title: '修改并重试', detail: '可以直接修改渠道模型或 Key，保存后重新检测验证。' })
  return tips
}

function formatLatencySec(ms: number | null | undefined): string {
  if (ms == null) return '超时'
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(2)}s`
}
</script>

<template>
  <div class="m26-modal-body m26-diagnostic-body">
    <div class="m26-diag-card danger">
      <div class="m26-diag-card-title">
        <span class="m26-diag-dot danger"></span>
        <span>故障摘要</span>
        <span class="m26-diag-code-pill">{{ extractErrorCode(props.channel.lastError) }}</span>
      </div>
      <div class="m26-diag-main-msg">{{ formatErrorSummary(props.channel.lastError) }}</div>
    </div>

    <div class="m26-diag-meta-grid">
      <div class="m26-diag-meta-item"><span class="label">探活主模型</span><span class="value font-mono">{{ props.channel.modelNames?.[0] || '默认模型' }}</span></div>
      <div class="m26-diag-meta-item"><span class="label">接口协议</span><span class="value font-mono">{{ props.channel.protocol }}</span></div>
      <div class="m26-diag-meta-item"><span class="label">上游返回耗时</span><span class="value font-mono">{{ props.channel.latency ? formatLatencySec(props.channel.latency) : '超时' }}</span></div>
      <div class="m26-diag-meta-item"><span class="label">最近检测时间</span><span class="value">{{ props.channel.lastCheck }}</span></div>
    </div>

    <div class="m26-diag-card advice">
      <div class="m26-diag-card-title">
        <span class="m26-diag-dot info"></span>
        <span>智能排查与修复建议</span>
      </div>
      <div class="m26-diag-advice-list">
        <div v-for="(tip, i) in getTroubleshootingAdvice(props.channel)" :key="i" class="m26-diag-tip-item">
          <span class="tip-num">{{ i + 1 }}</span>
          <span class="tip-txt"><strong>{{ tip.title }}</strong>：{{ tip.detail }}</span>
        </div>
      </div>
    </div>

    <div class="m26-diag-raw-section">
      <div class="m26-diag-raw-header">
        <span class="label">上游原始回包数据 (Raw Response)</span>
        <button class="m26-text-btn" @click="emit('copy-raw', props.channel.lastError || '')">复制完整报错</button>
      </div>
      <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ props.channel.lastError || '无原始回包' }}</code></pre>
    </div>
  </div>
</template>

<style scoped>
.m26-diagnostic-body { padding: 20px 24px; display: flex; flex-direction: column; gap: 16px; max-height: calc(88vh - 130px); overflow-y: auto; }
.m26-diag-card { padding: 14px 16px; border-radius: 12px; display: flex; flex-direction: column; gap: 8px; }
.m26-diag-card.danger { background: rgba(225, 29, 72, 0.05); border: 1px solid rgba(225, 29, 72, 0.2); }
.m26-diag-card.advice { background: #f8fafc; border: 1px solid var(--m26-border); }
.m26-diag-card-title { display: flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 700; color: var(--m26-text); }
.m26-diag-dot { width: 7px; height: 7px; border-radius: 50%; }
.m26-diag-dot.danger { background: var(--m26-rose); box-shadow: 0 0 6px rgba(225, 29, 72, 0.5); }
.m26-diag-dot.info { background: var(--m26-accent); box-shadow: 0 0 6px rgba(79, 70, 229, 0.5); }
.m26-diag-code-pill { margin-left: auto; font-family: ui-monospace, monospace; font-size: 11px; padding: 1px 7px; border-radius: 4px; background: rgba(225, 29, 72, 0.12); color: var(--m26-rose); font-weight: 700; }
.m26-diag-main-msg { font-size: 13px; font-weight: 600; color: #1e293b; line-height: 1.5; }
.m26-diag-meta-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px; }
.m26-diag-meta-item { padding: 10px 12px; background: #f8fafc; border: 1px solid var(--m26-border); border-radius: 8px; display: flex; flex-direction: column; gap: 4px; }
.m26-diag-meta-item .label, .m26-diag-raw-header .label { font-size: 11px; color: var(--m26-text-muted); }
.m26-diag-meta-item .value { font-size: 12px; font-weight: 600; color: var(--m26-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.m26-diag-advice-list { display: flex; flex-direction: column; gap: 8px; }
.m26-diag-tip-item { display: flex; gap: 10px; align-items: flex-start; font-size: 12px; line-height: 1.5; color: #334155; }
.m26-diag-tip-item .tip-num { width: 18px; height: 18px; border-radius: 50%; background: rgba(79, 70, 229, 0.1); color: var(--m26-accent); font-weight: 700; font-size: 11px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; margin-top: 1px; }
.m26-diag-raw-section { display: flex; flex-direction: column; gap: 6px; }
.m26-diag-raw-header { display: flex; justify-content: space-between; align-items: center; }
.m26-diag-raw-header .label { font-weight: 600; }
.m26-diag-raw-code { padding: 10px 14px; background: #0f172a; color: #f1f5f9; border-radius: 8px; max-height: 160px; overflow-y: auto; white-space: pre-wrap; word-break: break-all; line-height: 1.4; }
@media (max-width: 640px) { .m26-diag-meta-grid { grid-template-columns: 1fr; } }
</style>
