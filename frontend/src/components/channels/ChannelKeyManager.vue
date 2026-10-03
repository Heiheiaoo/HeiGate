<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { keyRequest, revealKey, type ChannelKey } from '@/services/channelKeys'

const props = defineProps<{
  channels: Array<{ gatewayId?: number; name: string; enabled?: boolean }>
  fixedChannelId?: number
}>()
const emit = defineEmits<{ changed: [] }>()
const channelId = ref<number | undefined>(props.fixedChannelId ?? (props.channels.find(c => c.gatewayId && c.enabled !== false) ?? props.channels.find(c => c.gatewayId))?.gatewayId)
const channelDisabled = computed(() => props.channels.find(c => c.gatewayId === channelId.value)?.enabled === false)
const keys = ref<ChannelKey[]>([])
const busy = ref(false)
const error = ref('')
const deleteId = ref<number | null>(null)
const revealedKeys = ref<Record<number, string>>({})
const revealingId = ref<number | null>(null)
const copiedId = ref<number | null>(null)
const copyResetTimers = new Map<number, number>()
onBeforeUnmount(() => {
  for (const timer of copyResetTimers.values()) window.clearTimeout(timer)
  copyResetTimers.clear()
})
const isAddingRow = ref(false)
const newKeyName = ref('')
const newKeySecret = ref('')

async function run(method = 'GET', body?: unknown, keyId?: number) {
  const id = channelId.value
  if (id === undefined || busy.value) return false
  busy.value = true
  error.value = ''
  try {
    const result = await keyRequest(id, method, body, keyId)
    if (id === channelId.value) keys.value = result
    if (method !== 'GET') emit('changed')
    return true
  } catch (err) {
    error.value = err instanceof Error ? err.message : '请求失败'
    return false
  } finally {
    busy.value = false
  }
}

watch(channelId, () => {
  keys.value = []
  deleteId.value = null
  revealedKeys.value = {}
  copiedId.value = null
  isAddingRow.value = false
  newKeyName.value = ''
  newKeySecret.value = ''
  void run()
}, { immediate: true })

async function remove() {
  if (deleteId.value === null) return
  if (await run('DELETE', undefined, deleteId.value)) deleteId.value = null
}

async function loadSecret(key: ChannelKey) {
  if (revealedKeys.value[key.id] !== undefined || channelId.value === undefined || revealingId.value !== null) return revealedKeys.value[key.id] ?? ''
  revealingId.value = key.id
  error.value = ''
  try {
    const secret = await revealKey(channelId.value, key.id)
    revealedKeys.value = { ...revealedKeys.value, [key.id]: secret }
    return secret
  } catch (err) {
    error.value = err instanceof Error ? err.message : '读取 Key 失败'
    return ''
  } finally {
    revealingId.value = null
  }
}

async function toggleSecret(key: ChannelKey) {
  if (revealedKeys.value[key.id] !== undefined) {
    const next = { ...revealedKeys.value }
    delete next[key.id]
    revealedKeys.value = next
    return
  }
  await loadSecret(key)
}

async function copySecret(key: ChannelKey) {
  const secret = await loadSecret(key)
  if (!secret) return
  try {
    await navigator.clipboard.writeText(secret)
  } catch {
    const textarea = document.createElement('textarea')
    textarea.value = secret
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    document.execCommand('copy')
    textarea.remove()
  }
  copiedId.value = key.id
  const previousTimer = copyResetTimers.get(key.id)
  if (previousTimer !== undefined) window.clearTimeout(previousTimer)
  const timer = window.setTimeout(() => {
    if (copiedId.value === key.id) copiedId.value = null
    copyResetTimers.delete(key.id)
  }, 1600)
  copyResetTimers.set(key.id, timer)
}

async function onNameChange(key: ChannelKey, newName: string, event?: Event) {
  const trimmed = newName.trim()
  if (!trimmed || trimmed === key.name) {
    if (event?.target) (event.target as HTMLInputElement).value = key.name
    return
  }
  if (!(await run('PUT', { name: trimmed }, key.id)) && event?.target) {
    (event.target as HTMLInputElement).value = key.name
  }
}

function onSecretFocus(e: FocusEvent) {
  const target = e.target as HTMLInputElement
  target.select()
}

async function onSecretChange(key: ChannelKey, newSecret: string, event?: Event) {
  const target = event?.target as HTMLInputElement | undefined
  const trimmed = newSecret.trim()
  if (!trimmed || trimmed === key.masked_secret || trimmed.includes('••••')) {
    if (target) target.value = revealedKeys.value[key.id] ?? key.masked_secret
    return
  }
  if (trimmed === revealedKeys.value[key.id]) return
  if (await run('PUT', { name: key.name, secret: trimmed }, key.id)) {
    revealedKeys.value = { ...revealedKeys.value, [key.id]: trimmed }
  } else if (target) {
    target.value = revealedKeys.value[key.id] ?? key.masked_secret
  }
}

function startAddKey() {
  isAddingRow.value = true
  newKeyName.value = `Key ${keys.value.length + 1}`
  newKeySecret.value = ''
}

function cancelNewKey() {
  isAddingRow.value = false
  newKeyName.value = ''
  newKeySecret.value = ''
}

async function submitNewKey() {
  const secret = newKeySecret.value.trim()
  if (!secret || /[\r\n]/.test(secret)) return
  const draft = {
    name: newKeyName.value.trim() || `Key ${keys.value.length + 1}`,
    secret,
  }
  if (await run('POST', draft)) {
    cancelNewKey()
  }
}
</script>

<template>
  <div class="key-manager">
    <div class="key-embedded-container">
      <div class="key-inputs-header">
        <div class="key-header-left">
          <label class="m26-label">API Key</label>
          <span class="key-note-badge">更改立即生效</span>
        </div>
        <span class="key-count-hint">已添加 {{ keys.length }} 把 Key</span>
      </div>

      <p v-if="channelDisabled" class="key-error">当前渠道已停用，Key 不会参与转发或测试。</p>
      <p v-if="error" role="alert" class="key-error">{{ error }}</p>

      <div class="key-cols-hint" aria-hidden="true">
        <span class="key-col-name">名称</span>
        <span class="key-col-secret">Key</span>
        <span class="key-col-action"></span>
      </div>

      <div v-for="key in keys" :key="key.id" class="key-input-row">
        <input
          :value="key.name"
          type="text"
          class="m26-input key-name-input text-xs"
          placeholder="名称"
          :disabled="busy"
          @change="onNameChange(key, ($event.target as HTMLInputElement).value, $event)"
        />

        <div class="key-secret-field">
          <input
            :value="revealedKeys[key.id] ?? key.masked_secret"
            type="text"
            class="m26-input key-secret-input font-mono text-xs"
            placeholder="sk-..."
            :disabled="busy"
            :title="revealedKeys[key.id] ? '已显示完整密钥，可直接修改' : '点击输入框可直接粘贴新密钥，失焦或回车自动更新'"
            @focus="onSecretFocus"
            @change="onSecretChange(key, ($event.target as HTMLInputElement).value, $event)"
            @keydown.enter="($event.target as HTMLInputElement).blur()"
          />

          <div class="key-field-tools">
            <button
              type="button"
              class="key-tool-btn"
              :title="revealedKeys[key.id] ? '隐藏 Key' : '查看完整 Key'"
              :disabled="busy || revealingId === key.id"
              @click="toggleSecret(key)"
            >
              <svg v-if="!revealedKeys[key.id]" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8">
                <path d="M1.5 8s2.5-4.5 6.5-4.5S14.5 8 14.5 8s-2.5 4.5-6.5 4.5S1.5 8 1.5 8z" />
                <circle cx="8" cy="8" r="2" />
              </svg>
              <svg v-else viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8">
                <path d="M1.5 8s2.5-4.5 6.5-4.5S14.5 8 14.5 8s-2.5 4.5-6.5 4.5S1.5 8 1.5 8z" />
                <circle cx="8" cy="8" r="2" />
                <path d="M2.5 2.5l11 11" stroke-linecap="round" />
              </svg>
            </button>
            <button
              type="button"
              class="key-tool-btn"
              :title="copiedId === key.id ? '已复制到剪贴板' : '复制完整 Key'"
              :disabled="busy || revealingId === key.id"
              @click="copySecret(key)"
            >
              <svg v-if="copiedId !== key.id" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8">
                <rect x="5" y="5" width="8" height="8" rx="1.5" />
                <path d="M3 11V3h8" stroke-linecap="round" />
              </svg>
              <span v-else class="key-copied-icon">✓</span>
            </button>
          </div>
        </div>

        <div class="key-col-action-cell">
          <template v-if="deleteId === key.id">
            <button
              type="button"
              class="key-del-btn is-danger-confirm"
              :disabled="busy"
              title="确定删除此 Key"
              @click="remove"
            >
              确认删除
            </button>
            <button
              type="button"
              class="key-del-cancel-btn"
              title="取消"
              @click="deleteId = null"
            >
              ✕
            </button>
          </template>
          <template v-else>
            <button
              type="button"
              class="key-del-btn"
              :disabled="busy || keys.length <= 1"
              :title="keys.length <= 1 ? '至少需要保留 1 把 Key' : '移除这把 Key'"
              @click="deleteId = key.id"
            >
              删除
            </button>
          </template>
        </div>
      </div>

      <!-- Add key row -->
      <div v-if="isAddingRow" class="key-input-row key-new-row">
        <input
          v-model="newKeyName"
          type="text"
          class="m26-input key-name-input text-xs"
          :placeholder="`Key ${keys.length + 1}`"
        />
        <div class="key-secret-field">
          <input
            v-model="newKeySecret"
            type="password"
            class="m26-input key-secret-input font-mono text-xs"
            placeholder="输入 sk-... 密钥"
            @keydown.enter.prevent="submitNewKey"
            @keydown.esc="cancelNewKey"
          />
        </div>
        <div class="key-new-row-actions">
          <button
            type="button"
            class="key-save-btn"
            :disabled="!newKeySecret.trim() || busy"
            @click="submitNewKey"
          >
            添加
          </button>
          <button
            type="button"
            class="key-cancel-btn"
            title="取消"
            @click="cancelNewKey"
          >
            ✕
          </button>
        </div>
      </div>

      <button
        v-else
        type="button"
        class="key-add-dashed-btn"
        :disabled="busy"
        @click="startAddKey"
      >
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M8 3v10M3 8h10" />
        </svg>
        <span>添加一把 Key</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.key-manager { width: 100%; min-width: 0; }
.key-error { color: #dc2626; font-size: 12px; }

/* Embedded styles matching ChannelKeyInputs */
.key-embedded-container {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
}

.key-inputs-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 2px;
}

.key-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
}

.key-note-badge {
  font-size: 10.5px;
  color: #64748b;
  background: #f1f5f9;
  padding: 1px 6px;
  border-radius: 4px;
}

.key-count-hint {
  font-size: 11px;
  color: #64748b;
}

.key-cols-hint {
  display: grid;
  grid-template-columns: 130px 1fr 88px;
  gap: 8px;
  font-size: 11px;
  color: #64748b;
  padding: 0 4px;
}

.key-col-name,
.key-col-secret,
.key-col-action {
  user-select: none;
}

.key-input-row {
  display: grid;
  grid-template-columns: 130px 1fr 88px;
  gap: 8px;
  align-items: center;
}

.key-name-input {
  width: 100% !important;
  min-width: 0 !important;
  height: 36px;
  border-radius: 8px;
  box-sizing: border-box;
}

.key-secret-field {
  position: relative;
  width: 100%;
  min-width: 0;
  display: flex;
  align-items: center;
}

.key-secret-field .key-secret-input {
  width: 100%;
  min-width: 0;
  height: 36px;
  padding-right: 58px;
  border-radius: 8px;
  box-sizing: border-box;
}

.key-secret-field .key-secret-input:focus,
.key-name-input:focus {
  border-color: #818cf8;
  box-shadow: 0 0 0 3px rgba(99, 102, 241, 0.12);
}

.key-field-tools {
  position: absolute;
  right: 5px;
  top: 50%;
  transform: translateY(-50%);
  display: flex;
  align-items: center;
  gap: 3px;
}

.key-tool-btn {
  width: 24px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: #64748b;
  cursor: pointer;
  padding: 0;
  transition: all 0.12s ease;
}

.key-tool-btn:hover:not(:disabled) {
  background: #f1f5f9;
  color: #1e293b;
}

.key-copied-icon {
  font-size: 11px;
  color: #10b981;
  font-weight: 700;
}

.key-col-action-cell {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 88px;
}

.key-del-btn {
  width: 100%;
  height: 36px;
  padding: 0 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  background: #ffffff;
  color: #64748b;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;
  box-sizing: border-box;
}

.key-del-btn:hover:not(:disabled) {
  border-color: #fca5a5;
  color: #dc2626;
  background: #fef2f2;
}

.key-del-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
  border-color: #f1f5f9;
}

.key-del-btn.is-danger-confirm {
  width: 62px;
  font-size: 11px;
  padding: 0;
  background: #dc2626;
  border-color: #dc2626;
  color: #ffffff;
  font-weight: 600;
}

.key-del-cancel-btn {
  width: 22px;
  height: 36px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  background: #ffffff;
  color: #64748b;
  font-size: 11px;
  cursor: pointer;
  padding: 0;
}

.key-del-cancel-btn:hover {
  background: #f1f5f9;
}

.key-new-row-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 88px;
}

.key-save-btn {
  flex: 1;
  height: 36px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: #0f172a;
  color: #ffffff;
  border: none;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.12s;
}

.key-save-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.key-cancel-btn {
  width: 28px;
  height: 36px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  background: #ffffff;
  color: #64748b;
  font-size: 11px;
  cursor: pointer;
}

.key-cancel-btn:hover {
  background: #f1f5f9;
}

.key-add-dashed-btn {
  width: 100%;
  height: 36px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border: 1px dashed #cbd5e1;
  border-radius: 8px;
  background: #f8fafc;
  color: #475569;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.15s ease;
  margin-top: 4px;
}

.key-add-dashed-btn:hover:not(:disabled) {
  border-color: #818cf8;
  background: #eef2ff;
  color: #4338ca;
}
</style>
