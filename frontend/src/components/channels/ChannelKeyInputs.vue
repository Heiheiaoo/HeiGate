<script setup lang="ts">
import { ref, watch } from 'vue'
const props = defineProps<{ modelValue: string }>()
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const rows = ref(props.modelValue ? props.modelValue.split('\n') : [''])
const batch = ref('')
const visible = ref(false)
watch(() => props.modelValue, value => {
  if (value !== rows.value.join('\n')) rows.value = value ? value.split('\n') : ['']
})
function update() { emit('update:modelValue', rows.value.join('\n')) }
function remove(index: number) { rows.value.splice(index, 1); update() }
function appendBatch() {
  rows.value = [...new Set([...rows.value, ...batch.value.split(/\r?\n/)].map(s => s.trim()).filter(Boolean))]
  if (!rows.value.length) rows.value = ['']
  batch.value = ''
  update()
}
</script>

<template>
  <section class="key-inputs" aria-label="API 密钥列表">
    <label class="m26-label">API Key</label>
    <div v-for="(_, index) in rows" :key="index" class="key-input-row">
      <input v-model="rows[index]" :aria-label="`Key ${index + 1}`" :type="visible ? 'text' : 'password'" class="m26-input" placeholder="sk-..." autocomplete="new-password" @input="update" />
      <button type="button" class="m26-glass-btn" :disabled="rows.length === 1" :aria-label="`移除 Key ${index + 1}`" @click="remove(index)">移除</button>
    </div>
    <div class="key-input-row">
      <button type="button" class="m26-glass-btn" @click="rows.push(''); update()">添加一把 Key</button>
      <button type="button" class="m26-glass-btn" @click="visible = !visible">{{ visible ? '隐藏密钥' : '显示密钥' }}</button>
    </div>
    <details>
      <summary>批量添加</summary>
      <textarea v-model="batch" aria-label="批量密钥" placeholder="每行一把 Key" class="m26-input" rows="3" autocomplete="off" spellcheck="false" />
      <button type="button" class="m26-glass-btn" :disabled="!batch.trim()" @click="appendBatch">加入列表并去重</button>
    </details>
  </section>
</template>

<style scoped>
.key-inputs { display: grid; gap: 10px; }
.key-input-row { display: flex; gap: 8px; align-items: center; }
.key-input-row input { min-width: 0; flex: 1; }
.key-inputs textarea { height: auto; margin: 8px 0; }
.key-inputs details { width: 100%; }
.key-inputs details textarea { display: block; width: 100%; }
.key-inputs small { color: #64748b; }
.key-inputs summary { cursor: pointer; font-size: 13px; }
</style>
