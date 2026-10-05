<script setup lang="ts">
import { computed, ref } from 'vue'

const props = defineProps<{
  mode: 'classify' | 'manage'
  sourceSuggestions: string[]
  targetOptions: string[]
  initialSource?: string
  initialTarget?: string
  mappings?: Record<string, string>
}>()

const emit = defineEmits<{
  (e: 'submit', value: { source: string; target: string }): void
  (e: 'remove', source: string): void
  (e: 'close'): void
}>()

const source = ref(props.initialSource || '')
const selectedTarget = ref(props.initialTarget || props.targetOptions[0] || '')
const customTarget = ref('')

const target = computed(() => (
  selectedTarget.value === '__custom__' ? customTarget.value.trim() : selectedTarget.value.trim()
))
const canSubmit = computed(() => Boolean(source.value.trim() && target.value))

function submit() {
  if (!canSubmit.value) return
  emit('submit', { source: source.value.trim(), target: target.value })
  if (props.mode === 'manage') {
    source.value = ''
    customTarget.value = ''
    if (props.targetOptions && props.targetOptions.length > 0) {
      selectedTarget.value = props.targetOptions[0]
    }
  }
}
</script>

<template>
  <div class="m26-modal-sheet mapping-sheet" :class="`mode-${props.mode}`">
    <!-- Header -->
    <div class="m26-modal-header">
      <div class="mapping-title">
        <div class="mapping-icon-badge">
          <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 6h16M4 12h10M4 18h14" />
          </svg>
        </div>
        <h3 :id="props.mode === 'classify' ? 'm26-modal-title-6' : 'm26-modal-title-7'" class="m26-modal-title">
          模型归并
        </h3>
      </div>
      <button class="m26-close-btn" aria-label="关闭" @click="emit('close')">✕</button>
    </div>

    <!-- Body -->
    <div class="m26-modal-body mapping-body">
      <!-- Manage Mode: Add Mapping Rule Box (Fixed single-line toolbar) -->
      <div v-if="props.mode === 'manage'" class="mapping-inline-bar">
        <div class="mapping-field source-field">
          <input
            v-model="source"
            type="text"
            class="m26-input font-mono"
            placeholder="原始上游模型 (例如: deepseek-v4-flash-0731-free-3)"
            :list="`mapping-source-${props.mode}`"
          />
          <datalist :id="`mapping-source-${props.mode}`">
            <option v-for="name in props.sourceSuggestions" :key="name" :value="name" />
          </datalist>
        </div>

        <div class="mapping-arrow-col">
          <span class="mapping-arrow">➔</span>
        </div>

        <div class="mapping-field target-field">
          <div class="flex items-center gap-1.5 w-full">
            <select v-model="selectedTarget" class="m26-select font-mono">
              <option v-for="name in props.targetOptions" :key="name" :value="name">{{ name }}</option>
              <option value="__custom__">+ 新目标模型...</option>
            </select>
            <input
              v-if="selectedTarget === '__custom__'"
              v-model="customTarget"
              type="text"
              class="m26-input font-mono custom-target"
              placeholder="目标模型规范名称"
              autofocus
            />
          </div>
        </div>

        <div class="mapping-action-col">
          <button
            class="m26-action-btn primary add-button"
            :disabled="!canSubmit"
            @click="submit"
          >
            添加
          </button>
        </div>
      </div>

      <!-- Classify Mode: Compact Form -->
      <div v-else class="classify-form">
        <div class="mapping-field">
          <label class="m26-label">待归并原始模型</label>
          <input
            v-model="source"
            type="text"
            class="m26-input font-mono"
            placeholder="例如: deepseek-v4-flash-0731-free-3"
            :list="`mapping-source-${props.mode}`"
          />
          <datalist :id="`mapping-source-${props.mode}`">
            <option v-for="name in props.sourceSuggestions" :key="name" :value="name" />
          </datalist>
        </div>

        <div class="mapping-field">
          <label class="m26-label">归入目标聚合模型</label>
          <div class="flex flex-col gap-1.5">
            <select v-model="selectedTarget" class="m26-select font-mono">
              <option v-for="name in props.targetOptions" :key="name" :value="name">{{ name }}</option>
              <option value="__custom__">+ 输入新目标模型名称...</option>
            </select>
            <input
              v-if="selectedTarget === '__custom__'"
              v-model="customTarget"
              type="text"
              class="m26-input font-mono custom-target"
              placeholder="目标模型规范名称"
              autofocus
            />
          </div>
        </div>
      </div>

      <!-- Manage Mode: Rules List (Takes all remaining window space) -->
      <div v-if="props.mode === 'manage'" class="mapping-rules-section">
        <div v-if="props.mappings && Object.keys(props.mappings).length" class="m26-custom-mappings-table">
          <div class="m26-mapping-row header">
            <span class="col-source">原始上游模型名</span>
            <span class="col-arrow"></span>
            <span class="col-target">归入标准模型</span>
            <span class="col-action">操作</span>
          </div>
          <div class="m26-mapping-list-scroll">
            <div v-for="(mappedTarget, raw) in props.mappings" :key="raw" class="m26-mapping-row">
              <span class="col-source font-mono" :title="String(raw)">{{ raw }}</span>
              <span class="col-arrow">➔</span>
              <span class="col-target">
                <span class="target-tag font-mono" :title="String(mappedTarget)">{{ mappedTarget }}</span>
              </span>
              <span class="col-action">
                <button
                  class="remove-button"
                  :aria-label="`解除 ${String(raw)} 的归类映射`"
                  title="解除此归类映射"
                  @click="emit('remove', String(raw))"
                >
                  <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                    <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6" />
                  </svg>
                </button>
              </span>
            </div>
          </div>
          <div class="m26-mapping-table-footer">
            <span class="rules-count-text">共 {{ Object.keys(props.mappings).length }} 条归类规则</span>
          </div>
        </div>

        <div v-else class="mapping-empty">
          <div class="mapping-empty-icon">
            <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M4 6h16M4 12h10M4 18h14" />
            </svg>
          </div>
          <div class="mapping-empty-text">暂无自定义归类规则</div>
          <div class="mapping-empty-sub">在上方添加规则后，上游原始模型将即时归并合并至统一标准卡片</div>
        </div>
      </div>
    </div>

    <!-- Footer (classify mode only) -->
    <div v-if="props.mode === 'classify'" class="m26-modal-footer">
      <button class="m26-glass-btn" @click="emit('close')">取消</button>
      <button class="m26-action-btn primary" :disabled="!canSubmit" @click="submit">确认归并</button>
    </div>
  </div>
</template>

<style scoped>
.mapping-sheet {
  max-width: 460px;
  width: 100%;
}
.mapping-sheet.mode-manage {
  max-width: 860px;
  width: 92%;
  height: 82vh;
  max-height: 82vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.mapping-sheet.mode-manage .mapping-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 14px 20px 16px;
  gap: 12px;
}
.m26-modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 20px;
  border-bottom: 1px solid var(--m26-border);
  min-height: 48px;
  flex-shrink: 0;
}
.mapping-title {
  display: flex;
  align-items: center;
  gap: 8px;
}
.mapping-icon-badge {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 6px;
  background: rgba(99, 102, 241, 0.08);
  color: var(--m26-accent, #6366f1);
}
.m26-modal-title {
  font-size: 14px;
  font-weight: 700;
  color: var(--m26-text);
  margin: 0;
}
.m26-close-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  color: var(--m26-text-muted);
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-close-btn:hover {
  background: rgba(0, 0, 0, 0.05);
  color: var(--m26-text);
}
.mapping-body {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 16px 20px;
}
.mapping-inline-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  border: 1px solid var(--m26-border);
  border-radius: 10px;
  background: #f8fafc;
  flex-shrink: 0;
}
.mapping-field {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 5px;
}
.field-label, .m26-label {
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-secondary);
}
.mapping-arrow-col {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.mapping-arrow {
  color: var(--m26-text-muted);
  font-size: 13px;
}
.mapping-action-col {
  display: flex;
  align-items: center;
  flex-shrink: 0;
}
.classify-form {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.m26-input, .m26-select {
  width: 100%;
  height: 34px;
  border: 1px solid var(--m26-border-bright);
  border-radius: 7px;
  background: #fff;
  padding: 0 10px;
  color: var(--m26-text);
  font-size: 12px;
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}
.m26-input:focus, .m26-select:focus {
  border-color: var(--m26-accent, #6366f1);
  outline: none;
  box-shadow: 0 0 0 2px rgba(99, 102, 241, 0.12);
}
.custom-target {
  margin-top: 0;
}
.add-button {
  height: 34px;
  padding: 0 16px;
  white-space: nowrap;
}
.mapping-rules-section {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.m26-custom-mappings-table {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--m26-border);
  border-radius: 10px;
  background: #fff;
  overflow: hidden;
}
.m26-mapping-list-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  -ms-overflow-style: none;
}
.m26-mapping-list-scroll::-webkit-scrollbar {
  display: none;
  width: 0;
  height: 0;
}
.m26-mapping-row {
  display: flex;
  align-items: center;
  padding: 9px 14px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.05);
  font-size: 12px;
  transition: background 0.1s ease;
}
.m26-mapping-row:hover:not(.header) {
  background: #fcfdfe;
}
.m26-mapping-row:last-child {
  border-bottom: 0;
}
.m26-mapping-row.header {
  flex-shrink: 0;
  background: #f8fafc;
  color: var(--m26-text-muted);
  font-size: 11.5px;
  font-weight: 600;
  border-bottom: 1px solid var(--m26-border);
}
.m26-mapping-table-footer {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding: 8px 14px;
  background: #f8fafc;
  border-top: 1px solid var(--m26-border);
}
.rules-count-text {
  font-size: 11.5px;
  font-weight: 550;
  color: var(--m26-text-muted);
}
.col-source {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--m26-text);
}
.col-arrow {
  flex: 0 0 28px;
  text-align: center;
  color: var(--m26-text-muted);
}
.col-target {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
}
.target-tag {
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--m26-accent, #6366f1);
  background: rgba(99, 102, 241, 0.08);
  border: 1px solid rgba(99, 102, 241, 0.15);
  border-radius: 5px;
  padding: 1px 7px;
  font-weight: 600;
  font-size: 11px;
}
.col-action {
  flex: 0 0 34px;
  text-align: right;
}
.remove-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border: 1px solid rgba(225, 29, 72, 0.2);
  border-radius: 6px;
  background: #fff;
  color: var(--m26-rose, #e11d48);
  cursor: pointer;
  transition: all 0.15s ease;
}
.remove-button:hover {
  background: #fff1f2;
  border-color: var(--m26-rose, #e11d48);
}
.mapping-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 28px 16px;
  border: 1px dashed var(--m26-border);
  border-radius: 8px;
  background: #fafbfe;
  text-align: center;
}
.mapping-empty-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: 50%;
  background: #f1f5f9;
  color: #94a3b8;
  margin-bottom: 8px;
}
.mapping-empty-text {
  font-size: 13px;
  font-weight: 600;
  color: var(--m26-text-secondary);
}
.mapping-empty-sub {
  font-size: 11px;
  color: var(--m26-text-muted);
  margin-top: 4px;
}
.m26-modal-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px;
  border-top: 1px solid var(--m26-border);
}
@media (max-width: 640px) {
  .mapping-controls {
    flex-direction: column;
    align-items: stretch;
  }
  .mapping-arrow-col {
    display: none;
  }
}
</style>
