<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { keyRequest, probeKey, revealKey, type ChannelKey } from '@/services/channelKeys'

const props = defineProps<{ channels: Array<{ gatewayId?: number; name: string; enabled?: boolean }>; fixedChannelId?: number; embedded?: boolean }>()
const emit = defineEmits<{ close: []; changed: [] }>()
const channelId = ref<number | undefined>(props.fixedChannelId ?? (props.channels.find(c => c.gatewayId && c.enabled !== false) ?? props.channels.find(c => c.gatewayId))?.gatewayId)
const channelDisabled = computed(() => props.channels.find(c => c.gatewayId === channelId.value)?.enabled === false)
const keys = ref<ChannelKey[]>([])
const busy = ref(false)
const error = ref('')
const batch = ref('')
const editId = ref<number | null>(null)
const editName = ref('')
const editSecret = ref('')
const deleteId = ref<number | null>(null)
const testIds = ref<number[]>([])
const revealedKeys = ref<Record<number, string>>({})
const revealingId = ref<number | null>(null)
const copiedId = ref<number | null>(null)
const now = ref(Date.now())
const timer = setInterval(() => { now.value = Date.now() }, 1000)
onBeforeUnmount(() => clearInterval(timer))
const readyCount = computed(() => keys.value.filter(k => k.enabled && !k.auth_invalid && (!k.cooldown_until || Date.parse(k.cooldown_until) <= now.value)).length)

function status(key: ChannelKey) {
  if (!key.enabled) return '已停用'
  if (key.auth_invalid) return '认证不可用，请替换 Key'
  const remaining = key.cooldown_until ? Math.ceil((Date.parse(key.cooldown_until) - now.value) / 1000) : 0
  if (remaining > 0) return `冷却 ${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, '0')}`
  return '可用'
}

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
  batch.value = ''
  editId.value = null
  deleteId.value = null
  revealedKeys.value = {}
  copiedId.value = null
  void run()
}, { immediate: true })

async function add() {
  const secrets = [...new Set(batch.value.split(/\r?\n/).map(s => s.trim()).filter(Boolean))]
  if (!secrets.length) return
  if (await run('POST', { keys: secrets.map((secret, i) => ({ name: `Key ${keys.value.length + i + 1}`, secret })) })) batch.value = ''
}

function edit(key: ChannelKey) {
  editId.value = key.id
  editName.value = key.name
  editSecret.value = ''
}

async function save() {
  if (editId.value === null) return
  if (await run('PUT', { name: editName.value, ...(editSecret.value.trim() ? { secret: editSecret.value.trim() } : {}) }, editId.value)) {
    editId.value = null
    editSecret.value = ''
  }
}

async function remove() {
  if (deleteId.value === null) return
  if (await run('DELETE', undefined, deleteId.value)) deleteId.value = null
}

async function testKeys() {
  if (channelId.value === undefined || busy.value) return
  busy.value = true
  error.value = ''
  try {
    for (const id of testIds.value) {
      const results = await probeKey(channelId.value, id)
      const failed = results.find(r => !['operational', 'degraded'].includes(r.status))
      if (failed) error.value = `${failed.key_name}：${failed.message}`
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '测试失败'
  } finally {
    testIds.value = []
    busy.value = false
    try { keys.value = await keyRequest(channelId.value) } catch { /* retain current results */ }
  }
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
  window.setTimeout(() => {
    if (copiedId.value === key.id) copiedId.value = null
  }, 1600)
}

</script>

<template>
  <div class="key-manager" :class="{ 'm26-modal-sheet': !embedded, 'key-manager-embedded': embedded }">
    <div v-if="!embedded" class="m26-modal-header">
      <h3 id="key-manager-title" class="m26-modal-title">供应商 Key 管理</h3>
      <button class="m26-close-btn" aria-label="关闭" @click="$emit('close')">✕</button>
    </div>
    <div :class="{ 'm26-modal-body': !embedded }">
      <h4 v-if="embedded" class="m26-label">API Key</h4>
      <p v-if="embedded" class="key-note">Key 更改立即生效</p>
      <label v-if="!embedded" class="m26-label" for="key-channel">选择供应商渠道</label>
      <select v-if="!embedded" id="key-channel" v-model="channelId" class="m26-select" :disabled="busy">
        <option v-for="channel in channels.filter(c => c.gatewayId)" :key="channel.gatewayId" :value="channel.gatewayId">{{ channel.name }}</option>
      </select>
      <p class="key-note">{{ keys.length }} 把 Key · {{ readyCount }} 可用</p>
      <p v-if="error" role="alert" class="key-error">{{ error }}</p>
      <p v-if="channelDisabled" class="key-error">当前渠道已停用，Key 不会参与转发或测试。</p>
      <div class="key-actions">
      <button class="m26-glass-btn" :disabled="busy" @click="run()">刷新状态</button>
      <button class="m26-glass-btn" :disabled="busy || channelDisabled || !readyCount" @click="testIds = keys.filter(k => status(k) === '可用').map(k => k.id)">测试全部可用 Key</button>
      </div>
      <div v-if="testIds.length" class="key-form" role="alert">
        <p>将向上游发送 {{ testIds.length }} 次主模型测试请求，可能消耗额度。冷却 Key 不会参与。</p>
        <button class="m26-glass-btn" :disabled="busy" @click="testKeys">确认测试</button>
        <button class="m26-glass-btn" :disabled="busy" @click="testIds = []">取消</button>
      </div>
      <div v-for="key in keys" :key="key.id" class="key-row">
        <div>
<strong>{{ key.name }}</strong> <code class="key-secret" :title="revealedKeys[key.id] ? '点击隐藏完整 Key' : '当前仅显示末尾字符'">{{ revealedKeys[key.id] ?? key.masked_secret }}</code><p>{{ status(key) }}</p>
          <small>选用 {{ key.attempts }} · 成功 {{ key.successes }} · 限流轮次 {{ key.rate_limits }} · Token {{ key.prompt_tokens + key.completion_tokens }}</small>
          <p v-if="key.last_error" class="key-error">{{ key.last_error }}</p>
        </div>
        <div class="key-actions">
          <button class="m26-glass-btn" :disabled="busy || channelDisabled || status(key) !== '可用'" @click="testIds = [key.id]">测试</button>
          <button class="m26-glass-btn" :disabled="busy || revealingId === key.id" @click="toggleSecret(key)">{{ revealingId === key.id ? '读取中…' : revealedKeys[key.id] !== undefined ? '隐藏 Key' : '显示完整' }}</button>
          <button class="m26-glass-btn" :disabled="busy || revealingId === key.id" @click="copySecret(key)">{{ copiedId === key.id ? '已复制' : '复制 Key' }}</button>
          <button class="m26-glass-btn" :disabled="busy" @click="run('PUT', { enabled: !key.enabled }, key.id)">{{ key.enabled ? '停用' : '启用' }}</button>
          <button class="m26-glass-btn" :disabled="busy" @click="edit(key)">编辑</button>
          <button class="m26-glass-btn" :disabled="busy" @click="run('PUT', { position: Math.min(...keys.map(k => k.position)) - 1 }, key.id)">置顶</button>
          <button class="m26-glass-btn danger" :disabled="busy || keys.length <= 1" @click="deleteId = key.id">删除</button>
        </div>
      </div>
      <form v-if="editId !== null" class="key-form" @submit.prevent="save">
        <label>Key 名称<input v-model="editName" class="m26-input" required /></label>
        <label>替换密钥（留空保留）<input v-model="editSecret" class="m26-input" type="password" autocomplete="new-password" /></label>
        <button class="m26-glass-btn" :disabled="busy || !editName.trim()">保存</button>
        <button type="button" class="m26-glass-btn" @click="editId = null; editSecret = ''">取消</button>
      </form>
      <div v-if="deleteId !== null" class="key-form" role="alert">
        <p>确定删除这把 Key？不会删除历史日志。此操作不可撤销。</p>
        <button class="m26-glass-btn danger" :disabled="busy" @click="remove">确认删除</button>
        <button class="m26-glass-btn" @click="deleteId = null">取消</button>
      </div>
      <form class="key-form" @submit.prevent="add">
        <label for="key-batch">添加 Key</label>
        <textarea placeholder="每行一把 Key" id="key-batch" v-model="batch" class="m26-input" rows="4" autocomplete="off" spellcheck="false" />
        <button class="m26-glass-btn" :disabled="busy || !batch.trim() || channelId === undefined">添加 Key</button>
      </form>
    </div>
  </div>
</template>

<style scoped>
.key-manager { max-width: 850px; width: min(850px, 95vw); }
.key-manager-embedded { width: 100%; min-width: 0; }
.key-manager .m26-modal-body { display: block; }
.key-manager .m26-glass-btn { width: auto; min-height: 32px; }
.key-form > button { justify-self: start; }
.key-note { margin: 12px 0; font-size: 12px; opacity: .7; }
.key-row { display: flex; justify-content: space-between; flex-wrap: wrap; gap: 12px; padding: 14px 0; border-bottom: 1px solid #8883; overflow-wrap: anywhere; }
.key-secret { word-break: break-all; }
.key-actions { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.key-form { display: grid; gap: 10px; padding: 16px 0; }
.key-form textarea { height: auto; min-height: 90px; }
.key-error { color: #dc2626; font-size: 12px; }
</style>
