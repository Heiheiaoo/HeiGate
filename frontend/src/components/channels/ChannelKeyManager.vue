<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { keyRequest, type ChannelKey } from '@/services/channelKeys'

const props = defineProps<{ channels: Array<{ gatewayId?: number; name: string }> }>()
defineEmits<{ close: []; changed: [] }>()
const channelId = ref<number | undefined>(props.channels.find(c => c.gatewayId)?.gatewayId)
const keys = ref<ChannelKey[]>([])
const busy = ref(false)
const error = ref('')
const batch = ref('')
const editId = ref<number | null>(null)
const editName = ref('')
const editSecret = ref('')
const deleteId = ref<number | null>(null)
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
</script>

<template>
  <div class="m26-modal-sheet key-manager">
    <div class="m26-modal-header">
      <h3 id="key-manager-title" class="m26-modal-title">供应商 Key 管理</h3>
      <button class="m26-close-btn" aria-label="关闭" @click="$emit('close')">✕</button>
    </div>
    <div class="m26-modal-body">
      <label class="m26-label" for="key-channel">选择供应商渠道</label>
      <select id="key-channel" v-model="channelId" class="m26-select" :disabled="busy">
        <option v-for="channel in channels.filter(c => c.gatewayId)" :key="channel.gatewayId" :value="channel.gatewayId">{{ channel.name }}</option>
      </select>
      <p class="key-note">{{ keys.length }} 把 Key · {{ readyCount }} 可用。轮询接入中；当前转发仍使用原渠道 Key。</p>
      <p v-if="error" role="alert" class="key-error">{{ error }}</p>
      <button class="m26-glass-btn" :disabled="busy" @click="run()">刷新状态</button>
      <div v-for="key in keys" :key="key.id" class="key-row">
        <div>
<strong>{{ key.name }}</strong> <code>{{ key.masked_secret }}</code><p>{{ status(key) }}</p>
          <small>尝试 {{ key.attempts }} · 成功 {{ key.successes }} · 429 {{ key.rate_limits }} · Token {{ key.prompt_tokens + key.completion_tokens }}</small>
          <p v-if="key.last_error" class="key-error">{{ key.last_error }}</p>
        </div>
        <div class="key-actions">
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
        <label for="key-batch">批量添加密钥（一行一把，最多 100 把）</label>
        <textarea id="key-batch" v-model="batch" class="m26-input" rows="4" autocomplete="off" spellcheck="false" />
        <button class="m26-glass-btn" :disabled="busy || !batch.trim() || channelId === undefined">添加 Key</button>
      </form>
    </div>
  </div>
</template>

<style scoped>
.key-manager { max-width: 850px; width: min(850px, 95vw); }
.key-note { margin: 12px 0; font-size: 12px; opacity: .7; }
.key-row { display: flex; justify-content: space-between; flex-wrap: wrap; gap: 12px; padding: 14px 0; border-bottom: 1px solid #8883; overflow-wrap: anywhere; }
.key-actions { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.key-form { display: grid; gap: 10px; padding: 16px 0; }
.key-form textarea { height: auto; min-height: 90px; }
.key-error { color: #dc2626; font-size: 12px; }
</style>
