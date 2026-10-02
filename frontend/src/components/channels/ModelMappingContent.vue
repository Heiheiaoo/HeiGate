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
  }
}
</script>

<template>
  <div class="m26-modal-sheet mapping-sheet" :class="`mode-${props.mode}`">
    <div class="m26-modal-header">
      <div class="mapping-title">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path d="M4 6h16M4 12h10M4 18h14" />
        </svg>
        <h3 :id="props.mode === 'classify' ? 'm26-modal-title-6' : 'm26-modal-title-7'" class="m26-modal-title">
          {{ props.mode === 'classify' ? '手动模型归并' : '自定义模型归类设置' }}
        </h3>
      </div>
      <button class="m26-close-btn" aria-label="关闭" @click="emit('close')">✕</button>
    </div>

    <div class="m26-modal-body mapping-body">
      <p class="mapping-description">
        {{ props.mode === 'classify'
          ? '将指定的原始模型（或特殊命名别名）归并到标准模型中。归入后，该渠道将与目标模型在同一卡片中聚合，并共享最低延迟调度与多渠道故障轮转。'
          : '管理手动归类映射，将上游非标准或私有模型名称重定向至标准模型。' }}
      </p>

      <div class="mapping-form" :class="{ compact: props.mode === 'manage' }">
        <span v-if="props.mode === 'manage'" class="mapping-form-title">添加新归类映射</span>
        <div class="mapping-controls">
          <div class="mapping-field source-field">
            <label v-if="props.mode === 'classify'" class="m26-label">待归并原始模型</label>
            <input
              v-model="source"
              type="text"
              class="m26-input font-mono"
              :placeholder="props.mode === 'classify' ? '例如: deepseek-v4-flash-0731-free-3' : '原始上游模型名'"
              :list="`mapping-source-${props.mode}`"
            />
            <datalist :id="`mapping-source-${props.mode}`">
              <option v-for="name in props.sourceSuggestions" :key="name" :value="name" />
            </datalist>
          </div>

          <span v-if="props.mode === 'manage'" class="mapping-arrow">➔</span>

          <div class="mapping-field target-field">
            <label v-if="props.mode === 'classify'" class="m26-label">归入目标聚合模型</label>
            <select v-model="selectedTarget" class="m26-select font-mono">
              <option v-for="name in props.targetOptions" :key="name" :value="name">{{ name }}</option>
              <option value="__custom__">+ {{ props.mode === 'classify' ? '输入新目标模型名称...' : '新目标模型...' }}</option>
            </select>
            <input
              v-if="selectedTarget === '__custom__'"
              v-model="customTarget"
              type="text"
              class="m26-input font-mono custom-target"
              placeholder="目标模型规范名称"
            />
          </div>

          <button v-if="props.mode === 'manage'" class="m26-action-btn primary add-button" :disabled="!canSubmit" @click="submit">添加</button>
        </div>
      </div>

      <div v-if="props.mode === 'manage' && Object.keys(props.mappings || {}).length" class="mapping-rules">
        <span class="mapping-rules-title">已生效规则 ({{ Object.keys(props.mappings || {}).length }})</span>
        <div class="m26-custom-mappings-table">
          <div class="m26-mapping-row header"><span class="col-source">原始上游模型名</span><span class="col-arrow"></span><span class="col-target">归入标准模型</span><span class="col-action">操作</span></div>
          <div v-for="(mappedTarget, raw) in props.mappings" :key="raw" class="m26-mapping-row">
            <span class="col-source font-mono">{{ raw }}</span><span class="col-arrow">➔</span><span class="col-target font-mono">{{ mappedTarget }}</span>
            <span class="col-action"><button class="remove-button" :aria-label="`解除 ${String(raw)} 的归类映射`" title="解除此归类映射" @click="emit('remove', String(raw))">✕</button></span>
          </div>
        </div>
      </div>
      <div v-else-if="props.mode === 'manage'" class="mapping-empty">暂无自定义归类规则。在上方添加后，卡片将即时合并。</div>
    </div>

    <div class="m26-modal-footer">
      <button v-if="props.mode === 'classify'" class="m26-glass-btn" @click="emit('close')">取消</button>
      <button v-if="props.mode === 'classify'" class="m26-action-btn primary" :disabled="!canSubmit" @click="submit">确认归并</button>
      <button v-else class="m26-action-btn primary" @click="emit('close')">完成</button>
    </div>
  </div>
</template>

<style scoped>
.mapping-sheet { max-width: 460px; }
.mapping-sheet.mode-manage { max-width: 580px; }
.mapping-title { display: flex; align-items: center; gap: 8px; }
.mapping-body { display: flex; flex-direction: column; gap: 14px; }
.mapping-description { margin: 0; color: var(--m26-text-muted); font-size: 12px; line-height: 1.6; }
.mapping-form { display: flex; flex-direction: column; gap: 10px; }
.mapping-form.compact { padding: 12px; border: 1px solid var(--m26-border); border-radius: 8px; background: #f8fafc; }
.mapping-form-title, .mapping-rules-title { color: var(--m26-text-secondary); font-size: 12px; font-weight: 700; }
.mapping-controls { display: flex; align-items: flex-end; gap: 8px; }
.mapping-field { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 6px; }
.mapping-sheet.mode-classify .mapping-controls { flex-direction: column; align-items: stretch; }
.mapping-arrow { align-self: center; color: var(--m26-text-muted); }
.m26-input, .m26-select { width: 100%; height: 34px; border: 1px solid var(--m26-border-bright); border-radius: 7px; background: #fff; padding: 0 9px; color: var(--m26-text); font-size: 12px; }
.custom-target { margin-top: 2px; }
.m26-label { color: var(--m26-text-secondary); font-size: 12px; font-weight: 600; }
.add-button { height: 34px; }
.mapping-rules { display: flex; flex-direction: column; gap: 7px; }
.m26-custom-mappings-table { overflow: hidden; border: 1px solid var(--m26-border); border-radius: 8px; background: #fff; }
.m26-mapping-row { display: flex; align-items: center; padding: 8px 12px; border-bottom: 1px solid var(--m26-border); font-size: 12px; }
.m26-mapping-row:last-child { border-bottom: 0; }
.m26-mapping-row.header { background: #f8fafc; color: var(--m26-text-muted); font-size: 11px; font-weight: 700; }
.col-source, .col-target { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.col-target { color: var(--m26-accent); font-weight: 600; }
.col-arrow { flex: 0 0 28px; text-align: center; color: var(--m26-text-muted); }
.col-action { flex: 0 0 34px; text-align: right; }
.remove-button { width: 24px; height: 24px; border: 1px solid rgba(225, 29, 72, 0.2); border-radius: 6px; color: var(--m26-rose); }
.mapping-empty { padding: 24px 12px; color: var(--m26-text-muted); font-size: 12px; text-align: center; }
@media (max-width: 640px) { .mapping-controls { flex-direction: column; align-items: stretch; } .mapping-arrow { display: none; } }
</style>
