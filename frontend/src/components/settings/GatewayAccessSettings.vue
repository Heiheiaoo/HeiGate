<script setup lang="ts">
defineProps<{
  gatewayBaseUrl: string
  gatewayApiKey: string
  maskedOverviewApiKey: string
  showApiKey: boolean
}>()

const emit = defineEmits<{
  'copy-url': []
  'copy-key': []
  'update:showApiKey': [value: boolean]
}>()
</script>

<template>
  <div class="m26-pref-group">
    <div class="m26-pref-group-header">
      <div class="m26-pref-group-icon icon-auth">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
          <path d="M7 11V7a5 5 0 0 1 10 0v4" />
        </svg>
      </div>
      <div class="m26-pref-group-titles">
        <h3 class="m26-pref-title">网关接入与本地密钥</h3>
        <span class="m26-pref-subtitle">本机分发服务的统一监听地址与第三方客户端身份校验凭证</span>
      </div>
    </div>

    <div class="m26-pref-card">
      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">网关 Base URL</span>
          <span class="desc">填入开发工具（如 Claude Code / Cursor / Codex CLI / Cline）中的 API 基础端点</span>
        </div>
        <div class="m26-pref-ctrl">
          <div class="m26-cred-field">
            <span class="m26-cred-field-tag">ENDPOINT</span>
            <span class="m26-cred-field-val font-mono" @click="emit('copy-url')" title="点击复制 Base URL">{{ gatewayBaseUrl }}</span>
            <button class="m26-cred-field-btn" @click="emit('copy-url')" title="复制 Base URL">
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
              </svg>
              <span>复制</span>
            </button>
          </div>
        </div>
      </div>

      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">网关安全密钥 (API Key)</span>
          <span class="desc">用于验证本地调用者身份（客户端发起请求时通过 Bearer Token 传输）</span>
        </div>
        <div class="m26-pref-ctrl">
          <div class="m26-cred-field">
            <span class="m26-cred-field-tag">SECRET</span>
            <span class="m26-cred-field-val font-mono" @click="emit('copy-key')" title="点击复制 API Key">
              {{ showApiKey ? (gatewayApiKey || '暂无 Key') : (maskedOverviewApiKey || '••••••••') }}
            </span>
            <div class="m26-cred-field-actions">
              <button
                class="m26-cred-icon-btn"
                @click="emit('update:showApiKey', !showApiKey)"
                :title="showApiKey ? '隐藏 API Key' : '显示完整 API Key'"
              >
                <svg v-if="!showApiKey" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                  <circle cx="12" cy="12" r="3" />
                </svg>
                <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
                  <line x1="1" y1="1" x2="23" y2="23" />
                </svg>
              </button>
              <button class="m26-cred-field-btn" @click="emit('copy-key')" title="复制 API Key">
                <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                </svg>
                <span>复制</span>
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

