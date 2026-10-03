<script setup lang="ts">
import { ref, computed, watch } from 'vue'

const props = defineProps<{
  primaryModel?: string
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

// Multi-line text for text mode
const rawModelText = ref('')

// Synchronize rawModelText when switching modes or when modelNames change
watch(
  () => props.modelNames,
  (newVal) => {
    if (!isTextMode.value) {
      rawModelText.value = (newVal || []).join('\n')
    }
    // Automatically keep primary model synced to first available model if unset or missing
    if (newVal && newVal.length > 0 && (!currentPrimary.value || !newVal.includes(currentPrimary.value))) {
      emit('update:primaryModel', newVal[0])
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
  if (!modelList.value.includes(m)) {
    emit('update:modelNames', [m, ...modelList.value])
  }
}
</script>

<template>
  <div class="m26-model-mgr-wrap" aria-label="模型配置">
    <!-- 可用模型池 -->
    <div class="m26-models-pool-section">
      <div class="m26-mgr-row-header">
        <div class="m26-mgr-left-group">
          <button
            type="button"
            class="m26-mgr-fetch-btn"
            :disabled="isFetching"
            title="从上游接口自动拉取模型列表"
            @click="emit('fetchUpstream')"
          >
            <span v-if="isFetching" class="m26-spin-dot mini"></span>
            <span>{{ isFetching ? '拉取中...' : '获取模型' }}</span>
          </button>
          <span class="m26-count-pill" title="已添加模型数量">{{ modelList.length }}</span>
        </div>
        <button
          type="button"
          class="m26-mode-toggle"
          @click="isTextMode = !isTextMode"
        >
          {{ isTextMode ? '标签模式' : '多行文本' }}
        </button>
      </div>

      <!-- 标签模式 -->
      <div v-if="!isTextMode" class="m26-tag-mode-wrap">
        <div class="m26-add-bar">
          <input
            v-model="newTagText"
            type="text"
            class="m26-mgr-input font-mono text-xs"
            placeholder="输入模型名称，按 Enter 添加（支持批量粘贴）"
            aria-label="添加模型"
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

        <div class="m26-tags-scroll-area">
          <div
            v-for="m in modelList"
            :key="m"
            class="m26-tag-capsule"
            :class="{ 'is-primary': m === currentPrimary }"
          >
            <span v-if="m === currentPrimary" class="tag-primary-badge">主模型</span>
            <button
              v-else
              type="button"
              class="tag-set-btn"
              title="点击设为主模型"
              @click.stop="selectPrimary(m)"
            >
              设为主模型
            </button>
            <span class="tag-text font-mono truncate" :title="m">{{ m }}</span>
            <button
              type="button"
              class="tag-del-btn"
              :aria-label="`移除模型 ${m}`"
              title="移除此模型"
              @click.stop="removeModel(m)"
            >
              ✕
            </button>
          </div>

          <div v-if="modelList.length === 0" class="m26-empty-tags-hint">
            暂无模型。可在此输入添加或点击上方“获取模型”
          </div>
        </div>
      </div>

      <!-- 多行文本模式 -->
      <div v-else class="m26-text-mode-wrap">
        <textarea
          :value="rawModelText"
          rows="4"
          class="m26-mgr-textarea font-mono text-xs"
          placeholder="每行一个模型名称，例如：&#10;gpt-4o&#10;gpt-4o-mini&#10;deepseek-chat"
          aria-label="多行模型列表"
          @input="onTextModeChange(($event.target as HTMLTextAreaElement).value)"
        ></textarea>
        <span class="m26-text-mode-hint">每行一个模型名称，重复项自动去重</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.m26-model-mgr-wrap {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 100%;
}

.m26-models-pool-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
}

.m26-mgr-row-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.m26-mgr-left-group {
  display: flex;
  align-items: center;
  gap: 8px;
}

.m26-count-pill {
  font-size: 11px;
  font-weight: 600;
  color: #64748b;
  background: #f1f5f9;
  padding: 1px 6px;
  border-radius: 9999px;
}

.m26-mgr-fetch-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 9px;
  font-size: 11px;
  font-weight: 600;
  color: #4338ca;
  background: #eef2ff;
  border: 1px solid #c7d2fe;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-mgr-fetch-btn:hover:not(:disabled) {
  background: #e0e7ff;
  border-color: #a5b4fc;
}

.m26-mgr-fetch-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.m26-mode-toggle {
  font-size: 11px;
  font-weight: 500;
  color: #6366f1;
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: color 0.12s;
}

.m26-mode-toggle:hover {
  color: #4338ca;
  text-decoration: underline;
}

.m26-tag-mode-wrap {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.m26-add-bar {
  display: flex;
  align-items: center;
  gap: 8px;
}

.m26-mgr-input {
  flex: 1;
  min-width: 0;
  width: 100%;
  height: 36px;
  padding: 0 10px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 8px;
  font-size: 12px;
  color: #0f172a;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s;
  box-sizing: border-box;
}

.m26-mgr-input:focus {
  border-color: #818cf8;
  box-shadow: 0 0 0 3px rgba(99, 102, 241, 0.12);
}

.m26-mgr-add-btn {
  height: 36px;
  padding: 0 14px;
  background: #0f172a;
  color: #ffffff;
  border: none;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.12s;
  flex-shrink: 0;
}

.m26-mgr-add-btn:hover:not(:disabled) {
  background: #1e293b;
}

.m26-mgr-add-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.m26-tags-scroll-area {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  max-height: 140px;
  overflow-y: auto;
  padding: 8px 10px;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  min-height: 48px;
  align-items: flex-start;
}

.m26-tag-capsule {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  padding: 3px 6px 3px 8px;
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
  background: #eef2ff;
  border-color: #818cf8;
  color: #312e81;
  font-weight: 600;
}

.tag-primary-badge {
  font-size: 9.5px;
  font-weight: 700;
  color: #4338ca;
  background: #e0e7ff;
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
  background: #e0e7ff;
  color: #4338ca;
}

.tag-text {
  max-width: 200px;
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
  color: #ef4444;
}

.m26-empty-tags-hint {
  font-size: 12px;
  color: #94a3b8;
  padding: 8px 4px;
  width: 100%;
  text-align: center;
}

.m26-text-mode-wrap {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-mgr-textarea {
  width: 100%;
  padding: 8px 10px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 8px;
  font-size: 12px;
  line-height: 1.6;
  color: #0f172a;
  outline: none;
  resize: vertical;
  transition: border-color 0.15s, box-shadow 0.15s;
  box-sizing: border-box;
}

.m26-mgr-textarea:focus {
  border-color: #818cf8;
  box-shadow: 0 0 0 3px rgba(99, 102, 241, 0.12);
}

.m26-text-mode-hint {
  font-size: 11px;
  color: #94a3b8;
}

.m26-spin-dot.mini {
  width: 10px;
  height: 10px;
  border: 1.5px solid #c7d2fe;
  border-top-color: #4338ca;
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
  display: inline-block;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
