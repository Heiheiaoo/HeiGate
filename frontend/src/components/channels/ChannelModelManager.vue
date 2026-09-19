<script setup lang="ts">
import { ref, computed, watch } from 'vue'

const props = defineProps<{
  primaryModel: string
  modelNames: string[]
  isFetching?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:primaryModel', val: string): void
  (e: 'update:modelNames', val: string[]): void
  (e: 'fetchUpstream'): void
}>()

// Current model list (synchronized with props.modelNames)
const modelList = computed({
  get: () => props.modelNames || [],
  set: (val: string[]) => emit('update:modelNames', val)
})

const currentPrimary = computed({
  get: () => props.primaryModel || '',
  set: (val: string) => emit('update:primaryModel', val)
})

// UI Mode: 'tags' or 'text'
const isTextMode = ref(false)

// Tag input field
const newTagText = ref('')

// Dropdown state for primary model quick selector
const isDropdownOpen = ref(false)

// Multi-line text for text mode
const rawModelText = ref('')

// Synchronize rawModelText when switching modes or when modelNames change
watch(
  () => props.modelNames,
  (newVal) => {
    if (!isTextMode.value) {
      rawModelText.value = (newVal || []).join('\n')
    }
  },
  { immediate: true }
)

function onTextModeChange(text: string) {
  rawModelText.value = text
  const parsed = text
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean)
  const unique = Array.from(new Set(parsed))
  emit('update:modelNames', unique)

  // If primary model is not set or removed, set first line as primary
  if (unique.length > 0 && (!currentPrimary.value || !unique.includes(currentPrimary.value))) {
    emit('update:primaryModel', unique[0])
  }
}

function parseAndAdd(input: string) {
  if (!input.trim()) return
  const items = input
    .split(/[,\n;\r\t]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  if (items.length === 0) return

  const current = [...modelList.value]
  let addedAny = false
  for (const item of items) {
    if (!current.includes(item)) {
      current.push(item)
      addedAny = true
    }
  }

  if (addedAny) {
    emit('update:modelNames', current)
    // If no primary model yet, set the first added model as primary
    if (!currentPrimary.value) {
      emit('update:primaryModel', current[0])
    }
  }
}

function addTag() {
  parseAndAdd(newTagText.value)
  newTagText.value = ''
}

function handlePaste(e: ClipboardEvent) {
  const paste = e.clipboardData?.getData('text')
  if (paste && (paste.includes('\n') || paste.includes(',') || paste.includes(';'))) {
    e.preventDefault()
    parseAndAdd(paste)
    newTagText.value = ''
  }
}

function removeModel(modelToRemove: string) {
  const updated = modelList.value.filter((m) => m !== modelToRemove)
  emit('update:modelNames', updated)

  // If the removed model was the primary model, reassign to the first remaining model
  if (currentPrimary.value === modelToRemove) {
    emit('update:primaryModel', updated[0] || '')
  }
}

function selectPrimary(m: string) {
  emit('update:primaryModel', m)
  // Ensure it is in modelNames
  if (!modelList.value.includes(m)) {
    emit('update:modelNames', [m, ...modelList.value])
  }
  isDropdownOpen.value = false
}

function onPrimaryInput(val: string) {
  const trimmed = val.trim()
  emit('update:primaryModel', trimmed)
  if (trimmed && !modelList.value.includes(trimmed)) {
    emit('update:modelNames', [trimmed, ...modelList.value])
  }
}
</script>

<template>
  <div class="m26-model-mgr-wrap">
    <!-- 1. Dedicated Primary Model Selection Field -->
    <div class="m26-primary-model-box">
      <div class="flex items-center justify-between mb-1.5">
        <label class="m26-mgr-label flex items-center gap-1.5">
          <span class="text-amber-500 font-bold">⚡</span>
          <span class="font-semibold text-slate-800">探活主模型</span>
          <span class="text-[11px] text-slate-400 font-normal">（用于周期探活与卡片首显）</span>
        </label>
        <span v-if="currentPrimary" class="m26-primary-chip truncate max-w-[180px]" :title="currentPrimary">
          当前：{{ currentPrimary }}
        </span>
      </div>

      <div class="relative">
        <div class="flex items-center gap-2">
          <div class="relative flex-1">
            <input
              type="text"
              :value="currentPrimary"
              class="m26-mgr-input font-mono text-xs pr-7"
              placeholder="输入或从下方点击选择主模型，例如：gpt-4o-mini"
              @input="onPrimaryInput(($event.target as HTMLInputElement).value)"
            />
            <button
              v-if="modelList.length > 0"
              type="button"
              class="absolute right-2 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-700 p-0.5"
              title="从已添加的模型中选择主模型"
              @click="isDropdownOpen = !isDropdownOpen"
            >
              <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M4 6l4 4 4-4" />
              </svg>
            </button>
          </div>
        </div>

        <!-- Quick Dropdown Menu -->
        <div v-if="isDropdownOpen && modelList.length > 0" class="m26-mgr-dropdown">
          <div
            v-for="m in modelList"
            :key="m"
            class="m26-mgr-dropdown-item"
            :class="{ 'is-selected': m === currentPrimary }"
            @click="selectPrimary(m)"
          >
            <span class="truncate font-mono">{{ m }}</span>
            <span v-if="m === currentPrimary" class="text-blue-600 text-[11px] font-semibold shrink-0">✓ 主模型</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 2. Models Pool / Tag Capsules -->
    <div class="m26-models-pool-section">
      <div class="flex justify-between items-center mb-2">
        <div class="flex items-center gap-2">
          <label class="m26-mgr-label mb-0">
            全部支持模型 <span class="text-slate-400 font-normal">({{ modelList.length }})</span>
          </label>
          <button
            type="button"
            class="text-[11px] text-blue-600 hover:text-blue-800 hover:underline cursor-pointer bg-transparent border-none p-0"
            @click="isTextMode = !isTextMode"
          >
            {{ isTextMode ? '切换为胶囊标签 🏷' : '切换为多行文本 ✎' }}
          </button>
        </div>

        <button
          type="button"
          class="m26-mgr-fetch-btn"
          :disabled="isFetching"
          @click="emit('fetchUpstream')"
        >
          <span v-if="isFetching" class="m26-spin-dot mini"></span>
          <span>{{ isFetching ? '拉取中...' : '自动拉取上游模型' }}</span>
        </button>
      </div>

      <!-- Tag Mode -->
      <div v-if="!isTextMode" class="m26-tag-mode-wrap">
        <!-- Input bar -->
        <div class="m26-add-bar">
          <input
            v-model="newTagText"
            type="text"
            class="m26-mgr-input font-mono text-xs flex-1"
            placeholder="输入模型名称，按 Enter 添加；支持直接粘贴多行或逗号文本..."
            @keydown.enter.prevent="addTag"
            @paste="handlePaste"
          />
          <button
            type="button"
            class="m26-mgr-add-btn"
            :disabled="!newTagText.trim()"
            @click="addTag"
          >
            添加
          </button>
        </div>

        <!-- Tags container -->
        <div class="m26-tags-scroll-area">
          <div
            v-for="m in modelList"
            :key="m"
            class="m26-tag-capsule"
            :class="{ 'is-primary': m === currentPrimary }"
          >
            <span v-if="m === currentPrimary" class="tag-primary-badge">
              ⚡ 主模型
            </span>
            <button
              v-else
              type="button"
              class="tag-set-btn"
              title="点击设为此渠道的探活主模型"
              @click.stop="selectPrimary(m)"
            >
              设为主模型
            </button>

            <span class="tag-text font-mono truncate" :title="m">{{ m }}</span>

            <button
              type="button"
              class="tag-del-btn"
              title="移除此模型"
              @click.stop="removeModel(m)"
            >
              ✕
            </button>
          </div>

          <div v-if="modelList.length === 0" class="m26-empty-tags-hint">
            暂无模型。可在此输入模型名称，或点击右上角“自动拉取上游模型”
          </div>
        </div>
      </div>

      <!-- Multi-line Textarea Mode -->
      <div v-else class="m26-text-mode-wrap">
        <textarea
          :value="rawModelText"
          rows="4"
          class="m26-mgr-textarea font-mono text-xs"
          placeholder="gpt-4o-mini&#10;gpt-4o&#10;deepseek-chat"
          @input="onTextModeChange(($event.target as HTMLTextAreaElement).value)"
        ></textarea>
        <p class="text-[11px] text-slate-400 mt-1 mb-0">
          * 每行一个模型名称。修改后首行将作为默认主模型，也可以在上方的“探活主模型”处随时指定。
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.m26-model-mgr-wrap {
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
}

.m26-primary-model-box {
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  border-radius: 10px;
  padding: 10px 12px;
}

.m26-mgr-label {
  font-size: 12px;
  font-weight: 600;
  color: #334155;
  user-select: none;
}

.m26-primary-chip {
  font-size: 11px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-weight: 600;
  color: #2563eb;
  background: #eff6ff;
  border: 1px solid rgba(37, 99, 235, 0.2);
  padding: 1px 7px;
  border-radius: 9999px;
}

.m26-mgr-input {
  width: 100%;
  height: 32px;
  padding: 0 10px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  font-size: 12px;
  color: #0f172a;
  outline: none;
  transition: all 0.12s;
  box-sizing: border-box;
}

.m26-mgr-input:focus {
  border-color: #3b82f6;
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.18);
}

.m26-mgr-dropdown {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 8px;
  box-shadow: 0 10px 25px rgba(0, 0, 0, 0.12);
  max-height: 180px;
  overflow-y: auto;
  z-index: 50;
  padding: 4px 0;
}

.m26-mgr-dropdown-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  font-size: 12px;
  color: #334155;
  cursor: pointer;
  transition: background 0.1s;
}

.m26-mgr-dropdown-item:hover {
  background: #f1f5f9;
}

.m26-mgr-dropdown-item.is-selected {
  background: #eff6ff;
  color: #1d4ed8;
  font-weight: 600;
}

.m26-models-pool-section {
  display: flex;
  flex-direction: column;
}

.m26-mgr-fetch-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: transparent;
  border: none;
  font-size: 11px;
  font-weight: 500;
  color: #2563eb;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: all 0.12s;
}

.m26-mgr-fetch-btn:hover:not(:disabled) {
  background: #eff6ff;
}

.m26-mgr-fetch-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.m26-add-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 8px;
}

.m26-mgr-add-btn {
  height: 32px;
  padding: 0 14px;
  background: #0f172a;
  color: #ffffff;
  border: none;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.12s;
  flex-shrink: 0;
}

.m26-mgr-add-btn:hover:not(:disabled) {
  background: #1e293b;
}

.m26-mgr-add-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.m26-tags-scroll-area {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  max-height: 140px;
  overflow-y: auto;
  padding: 8px;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  min-height: 52px;
}

.m26-tag-capsule {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  padding: 2.5px 6px 2.5px 8px;
  border-radius: 6px;
  font-size: 11.5px;
  color: #334155;
  max-width: 100%;
  transition: all 0.12s;
}

.m26-tag-capsule:hover {
  border-color: #94a3b8;
}

.m26-tag-capsule.is-primary {
  background: #eff6ff;
  border-color: #93c5fd;
  color: #1e40af;
}

.tag-primary-badge {
  font-size: 9.5px;
  font-weight: 700;
  color: #1d4ed8;
  background: #dbeafe;
  padding: 1px 5px;
  border-radius: 4px;
  flex-shrink: 0;
}

.tag-set-btn {
  font-size: 9.5px;
  font-weight: 500;
  color: #64748b;
  background: #f1f5f9;
  border: none;
  padding: 1.5px 5px;
  border-radius: 4px;
  cursor: pointer;
  flex-shrink: 0;
  transition: all 0.12s;
}

.tag-set-btn:hover {
  background: #dbeafe;
  color: #1d4ed8;
}

.tag-text {
  max-width: 180px;
}

.tag-del-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  background: transparent;
  border: none;
  border-radius: 50%;
  color: #94a3b8;
  cursor: pointer;
  font-size: 10px;
  padding: 0;
  transition: all 0.1s;
  flex-shrink: 0;
}

.tag-del-btn:hover {
  background: #fee2e2;
  color: #dc2626;
}

.m26-empty-tags-hint {
  font-size: 12px;
  color: #94a3b8;
  padding: 8px 4px;
}

.m26-mgr-textarea {
  width: 100%;
  padding: 8px 10px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  outline: none;
  box-sizing: border-box;
  resize: vertical;
}

.m26-mgr-textarea:focus {
  border-color: #3b82f6;
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.18);
}

.m26-spin-dot.mini {
  width: 10px;
  height: 10px;
  border: 1.5px solid rgba(59, 130, 246, 0.3);
  border-top-color: #3b82f6;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
  display: inline-block;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
