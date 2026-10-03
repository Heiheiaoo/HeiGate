<script setup lang="ts">
import type { ModelInjectionResult } from '@/services/gatewayApi'
import type { Channel } from './types'

const props = defineProps<{
  channel: Channel | null
  results: ModelInjectionResult[]
  checking: boolean
  copiedModelId: string
  copiedInjectionText: string
}>()

const emit = defineEmits<{
  (e: 'copy-model', modelName: string): void
  (e: 'copy-content', content: string): void
  (e: 'check-primary'): void
  (e: 'check-all'): void
  (e: 'close'): void
}>()

function modelInjectionLabel(result: ModelInjectionResult): string {
  switch (result.state) {
    case 'clean': return '输入正常'
    case 'suspected': return `输入偏高 · ${result.prompt_tokens} tok`
    case 'detected': return `显著偏高 · ${result.prompt_tokens} tok`
    case 'error': return '失败'
    default: return '未知'
  }
}

function injectionStateLabel(channel: Channel | null): string {
  switch (channel?.injectionState) {
    case 'clean': return '输入正常'
    case 'suspected': return `输入偏高 · ${channel.injectionTokens ?? '?'} tok`
    case 'detected': return `显著偏高 · ${channel.injectionTokens ?? '?'} tok`
    case 'error': return '检测失败'
    default: return '未检测'
  }
}
</script>

<template>
  <div class="m26-modal-sheet m26-inject-sheet">
    <div class="m26-modal-header">
      <div class="flex items-center gap-2.5">
        <h3 id="m26-modal-title-1" class="m26-modal-title">注入检测 — {{ props.channel?.name }}</h3>
        <span class="m26-inject-pill" :class="props.channel?.injectionState ?? 'unknown'">
          {{ injectionStateLabel(props.channel) }}
        </span>
      </div>
      <button class="m26-close-btn" aria-label="关闭" @click="emit('close')">✕</button>
    </div>

    <div class="m26-modal-body m26-inject-body">
      <div class="m26-inject-summary">
        <span class="m26-inject-meta">
          {{ props.channel?.injectionCheckedAt
            ? `上次检测时间: ${props.channel.injectionCheckedAt.slice(0, 19).replace('T', ' ')}`
            : '该渠道尚未执行过注入检测' }}
        </span>
      </div>
      <div class="m26-inject-desc-box">
        <p class="m26-inject-desc">
          检测会发送真实请求，并参考最小提问的输入 token 数（常见模板开销约 20–80）。超出参考范围只代表输入统计偏高，不能单独证明注入；明显超出阈值时会尝试提取提示内容，结果受模型回答影响。
        </p>
      </div>

      <div v-if="props.results.length" class="m26-inject-list">
        <div
          v-for="result in props.results"
          :key="result.model"
          class="m26-inject-row"
          :class="{ expanded: result.content && (result.state === 'detected' || result.state === 'suspected') }"
        >
          <div class="m26-inject-row-head">
            <div class="flex items-center gap-2 min-w-0 flex-1">
              <span class="m26-inject-model" :title="result.model">{{ result.model }}</span>
              <button class="m26-model-copy-btn" title="复制模型名称" @click.stop="emit('copy-model', result.model)">
                <svg v-if="props.copiedModelId !== result.model" viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                  <rect x="5" y="5" width="8" height="8" rx="1.5" />
                  <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                </svg>
                <svg v-else viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="#059669" stroke-width="2.2" aria-hidden="true">
                  <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                </svg>
                <span>{{ props.copiedModelId === result.model ? '已复制' : '复制' }}</span>
              </button>
            </div>
            <div class="flex items-center gap-2">
              <span v-if="result.prompt_tokens" class="m26-tokens-pill">{{ result.prompt_tokens }} tokens</span>
              <span class="m26-inject-pill" :class="result.state">{{ modelInjectionLabel(result) }}</span>
            </div>
          </div>

          <div v-if="result.content && (result.state === 'detected' || result.state === 'suspected')" class="m26-inject-content-container">
            <div class="m26-inject-content-toolbar">
              <div class="flex items-center gap-1.5">
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                  <path d="M2.5 3.5h11M2.5 6.5h11M2.5 9.5h7M2.5 12.5h5" />
                </svg>
                <span class="m26-inject-content-title">模型返回的提示词内容（需人工判断）:</span>
              </div>
              <button class="m26-model-copy-btn" title="复制完整注入 Prompt 内容" @click.stop="emit('copy-content', result.content)">
                <svg v-if="props.copiedInjectionText !== result.content" viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                  <rect x="5" y="5" width="8" height="8" rx="1.5" />
                  <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                </svg>
                <svg v-else viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="#059669" stroke-width="2.2" aria-hidden="true">
                  <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                </svg>
                <span>{{ props.copiedInjectionText === result.content ? '已复制 Prompt' : '复制 Prompt' }}</span>
              </button>
            </div>
            <pre class="m26-inject-content">{{ result.content }}</pre>
          </div>
          <div v-else-if="result.state === 'clean'" class="m26-inject-row-note">最小探针的输入 token 统计为 {{ result.prompt_tokens }}，处于正常模板参考范围内。</div>
          <div v-else-if="result.message" class="m26-inject-row-note">{{ result.message }}</div>
        </div>
      </div>
      <div v-else class="m26-inject-empty">
        <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" stroke-width="1.6" class="m26-empty-shield-icon" aria-hidden="true">
          <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
        </svg>
        <p>该渠道尚未执行注入检测，点击下方按钮开始探测。</p>
      </div>
    </div>

    <div class="m26-modal-footer m26-inject-footer">
      <div class="m26-inject-footer-left">
        <span v-if="props.checking" class="m26-inject-running-state">
          <span class="m26-spin-dot"></span>
          <span>正在向上游探测系统提示词注入，请稍候...</span>
        </span>
        <span v-else class="m26-inject-tip">真实上游请求 · 输入 token 数仅作参考，建议优先检测主模型</span>
      </div>
      <div class="m26-inject-footer-actions">
        <button class="m26-inject-btn secondary" :disabled="props.checking" title="仅检测主模型（推荐，速度快且省 Token）" @click="emit('check-primary')">
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
            <circle cx="8" cy="8" r="6" /><circle cx="8" cy="8" r="2" />
          </svg>
          <span>{{ props.checking ? '检测中...' : '检测主模型' }}</span>
        </button>
        <button class="m26-inject-btn primary" :disabled="props.checking" title="逐一检测全部模型（耗时可能较长）" @click="emit('check-all')">
          <span v-if="props.checking" class="m26-spin-dot white"></span>
          <svg v-else viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
            <rect x="2" y="2" width="5" height="5" rx="1" /><rect x="9" y="2" width="5" height="5" rx="1" /><rect x="2" y="9" width="5" height="5" rx="1" /><rect x="9" y="9" width="5" height="5" rx="1" />
          </svg>
          <span>{{ props.checking ? '全量检测中...' : '检测全部模型' }}</span>
        </button>
      </div>
    </div>
  </div>
</template>
