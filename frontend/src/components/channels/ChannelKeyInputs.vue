<script setup lang="ts">
import { ref, watch } from 'vue'

export type ChannelKeyDraft = {
  name: string
  secret: string
}

type DraftRow = ChannelKeyDraft & { id: number }

const props = defineProps<{ modelValue: ChannelKeyDraft[] }>()
const emit = defineEmits<{ 'update:modelValue': [value: ChannelKeyDraft[]] }>()

function defaultRow(index: number): ChannelKeyDraft {
  return { name: `Key ${index + 1}`, secret: '' }
}

let nextRowId = 1

function cloneRows(value: ChannelKeyDraft[]): DraftRow[] {
  return value.length
    ? value.map((row, index) => ({ id: nextRowId++, name: row.name ?? `Key ${index + 1}`, secret: row.secret ?? '' }))
    : [{ id: nextRowId++, ...defaultRow(0) }]
}

const rows = ref<DraftRow[]>(cloneRows(props.modelValue))
const visibleRows = ref(new Set<number>())

function sameRows(a: DraftRow[], b: ChannelKeyDraft[]) {
  return a.length === b.length && a.every((row, index) => row.name === (b[index].name ?? `Key ${index + 1}`) && row.secret === (b[index].secret ?? ''))
}

watch(() => props.modelValue, value => {
  const next = cloneRows(value)
  if (!sameRows(rows.value, value)) rows.value = next
}, { deep: true })

function update() {
  emit('update:modelValue', rows.value.map(({ name, secret }) => ({ name, secret })))
}

function addRow() {
  rows.value.push({ id: nextRowId++, ...defaultRow(rows.value.length) })
  update()
}

function isVisible(id: number) {
  return visibleRows.value.has(id)
}

function toggleVisibility(id: number) {
  const next = new Set(visibleRows.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  visibleRows.value = next
}

function remove(index: number) {
  const removed = rows.value[index]
  rows.value.splice(index, 1)
  if (removed) {
    const next = new Set(visibleRows.value)
    next.delete(removed.id)
    visibleRows.value = next
  }
  if (!rows.value.length) rows.value = [{ id: nextRowId++, ...defaultRow(0) }]
  update()
}

</script>

<template>
  <section class="key-inputs" aria-label="API 密钥列表">
    <div class="key-header-grid">
      <div class="key-header-col-key">
        <label class="m26-label">API Key</label>
      </div>
      <div class="key-header-col-remark">
        <label class="m26-label">备注</label>
        <span class="key-count-hint">已添加 {{ rows.length }} 把 Key</span>
      </div>
      <div class="key-header-col-action" aria-hidden="true"></div>
    </div>

    <div v-for="(row, index) in rows" :key="row.id" class="key-input-row">
      <!-- 1. API Key (放前面) -->
      <div class="key-secret-field">
        <input
          v-model="row.secret"
          :aria-label="`Key ${index + 1}`"
          :type="isVisible(row.id) ? 'text' : 'password'"
          class="m26-input key-secret-input font-mono text-xs"
          placeholder="sk-..."
          autocomplete="new-password"
          @input="update"
        />
        <button
          type="button"
          class="key-visibility-btn"
          :aria-label="isVisible(row.id) ? `隐藏 Key ${index + 1}` : `显示 Key ${index + 1}`"
          :title="isVisible(row.id) ? '隐藏密钥' : '显示密钥'"
          :aria-pressed="isVisible(row.id)"
          @click.stop="toggleVisibility(row.id)"
        >
          <svg v-if="!isVisible(row.id)" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
            <circle cx="12" cy="12" r="3" />
          </svg>
          <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
            <line x1="1" y1="1" x2="23" y2="23" />
          </svg>
        </button>
      </div>

      <!-- 2. 备注 (放后面) -->
      <input
        v-model="row.name"
        :aria-label="`Key ${index + 1} 名称`"
        type="text"
        class="m26-input key-name-input text-xs"
        placeholder="例如：主 Key、备用 Key"
        autocomplete="off"
        @input="update"
      />

      <!-- 3. 红色删除图标按钮 -->
      <button
        type="button"
        class="key-del-icon-btn"
        :disabled="rows.length === 1"
        :aria-label="`移除 Key ${index + 1}`"
        title="移除此 Key"
        @click="remove(index)"
      >
        <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.7">
          <path d="M2.5 4.5h11M5.5 4.5V3a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v1.5M6 7.5v4M10 7.5v4M4 4.5l.7 8.5a1.5 1.5 0 0 0 1.5 1.5h3.6a1.5 1.5 0 0 0 1.5-1.5l.7-8.5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <span class="sr-only">移除</span>
      </button>
    </div>

    <div class="key-actions-row">
      <button type="button" class="key-add-dashed-btn" @click="addRow">添加 Key</button>
    </div>
  </section>
</template>

<style scoped>
.key-inputs { display: flex; flex-direction: column; gap: 8px; width: 100%; }
.key-header-grid {
  display: grid;
  grid-template-columns: 1fr 220px 36px;
  gap: 12px;
  align-items: center;
  margin-bottom: 2px;
}
.key-header-col-key {
  display: flex;
  align-items: center;
}
.key-header-col-remark {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
.key-header-col-action {
  width: 36px;
  height: 1px;
}
.key-header-col-key .m26-label,
.key-header-col-remark .m26-label {
  margin-bottom: 0;
}
.key-count-hint { font-size: 11px; color: #64748b; font-weight: 500; }
.key-input-row {
  display: grid;
  grid-template-columns: 1fr 220px 36px;
  gap: 12px;
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
}
.key-secret-field .key-secret-input {
  width: 100%;
  min-width: 0;
  height: 36px;
  padding-right: 36px;
  border-radius: 8px;
  box-sizing: border-box;
}
.key-secret-field .key-secret-input:focus,
.key-name-input:focus {
  border-color: #818cf8;
  box-shadow: 0 0 0 3px rgba(99, 102, 241, 0.12);
}
.key-visibility-btn {
  position: absolute;
  right: 6px;
  top: 50%;
  width: 26px;
  height: 26px;
  padding: 3px;
  transform: translateY(-50%);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: #64748b;
  cursor: pointer;
  transition: all 0.15s ease;
}
.key-visibility-btn:hover { background: #eef2ff; color: #4338ca; }
.key-visibility-btn svg { width: 15px; height: 15px; }

.key-del-icon-btn {
  width: 36px;
  height: 36px;
  padding: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid #fee2e2;
  border-radius: 8px;
  background: #fef2f2;
  color: #ef4444;
  cursor: pointer;
  transition: all 0.15s ease;
  box-sizing: border-box;
  flex-shrink: 0;
}
.key-del-icon-btn:hover:not(:disabled) {
  border-color: #fca5a5;
  color: #dc2626;
  background: #fee2e2;
  transform: translateY(-0.5px);
}
.key-del-icon-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
  border-color: #f1f5f9;
  background: #f8fafc;
  color: #94a3b8;
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

.key-actions-row { margin-top: 4px; }
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
}
.key-add-dashed-btn:hover {
  border-color: #818cf8;
  background: #eef2ff;
  color: #4338ca;
}

@media (max-width: 640px) {
  .key-header-grid {
    grid-template-columns: 1fr;
    gap: 6px;
  }
  .key-input-row {
    grid-template-columns: 1fr;
    gap: 8px;
  }
  .key-del-icon-btn {
    width: 100%;
  }
}
</style>
