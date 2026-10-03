<script setup lang="ts">
withDefaults(defineProps<{
  newMappingSource?: string
  newMappingTarget?: string
  allDetectedChannelModels?: string[]
  existingCanonicalModelNames?: string[]
  customModelMappings?: Record<string, string>
  appVersion: string
  showMappings?: boolean
}>(), {
  newMappingSource: '',
  newMappingTarget: '',
  allDetectedChannelModels: () => [],
  existingCanonicalModelNames: () => [],
  customModelMappings: () => ({}),
  showMappings: true
})

const emit = defineEmits<{
  'update:newMappingSource': [value: string]
  'update:newMappingTarget': [value: string]
  'add-mapping': []
  'remove-mapping': [source: string]
}>()
</script>

<template>
  <div class="m26-pref-group" v-if="showMappings">
    <div class="m26-pref-group-header">
      <div class="m26-pref-group-icon icon-storage">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M4 6h16M4 12h10M4 18h14" />
        </svg>
      </div>
      <div class="m26-pref-group-titles">
        <h3 class="m26-pref-title">自定义模型归类与别名合并</h3>
        <span class="m26-pref-subtitle">手动将上游特殊或非规范命名的模型合并到标准模型中，统一聚合卡片与多渠道容灾调度</span>
      </div>
    </div>

    <div class="m26-pref-card">
      <!-- Add New Mapping Rule -->
      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">新建归类映射规则</span>
          <span class="desc">将渠道上报的原始模型（如 deepseek-v4-flash-0731-free-3）强制映射至目标聚合模型</span>
        </div>
        <div class="m26-pref-ctrl">
          <div class="flex items-center gap-2 flex-wrap">
            <div class="flex items-center gap-1.5">
              <span class="text-xs text-muted">原始模型:</span>
              <input
                :value="newMappingSource"
                @input="emit('update:newMappingSource', ($event.target as HTMLInputElement).value)"
                type="text"
                class="m26-input font-mono !h-[30px] !w-[220px] !text-xs"
                placeholder="例如: deepseek-v4-flash-0731-free-3"
                list="source-model-suggestions"
              />
              <datalist id="source-model-suggestions">
                <option v-for="m in allDetectedChannelModels" :key="m" :value="m" />
              </datalist>
            </div>
            <span class="text-xs text-muted">➔ 归入:</span>
            <div class="flex items-center gap-1.5">
              <input
                :value="newMappingTarget"
                @input="emit('update:newMappingTarget', ($event.target as HTMLInputElement).value)"
                type="text"
                class="m26-input font-mono !h-[30px] !w-[180px] !text-xs"
                placeholder="例如: deepseek-v4-flash"
                list="target-model-suggestions"
              />
              <datalist id="target-model-suggestions">
                <option v-for="m in existingCanonicalModelNames" :key="m" :value="m" />
              </datalist>
            </div>
            <button
              class="m26-action-btn primary !h-[30px] !text-xs"
              :disabled="!newMappingSource.trim() || !newMappingTarget.trim()"
              @click="emit('add-mapping')"
            >
              <span>添加归类</span>
            </button>
          </div>
        </div>
      </div>

      <!-- Existing Rules List -->
      <div class="m26-pref-row !block" v-if="Object.keys(customModelMappings).length > 0">
        <div class="w-full">
          <div class="m26-custom-mappings-table">
            <div class="m26-mapping-row header">
              <span class="col-source">原始上游模型名</span>
              <span class="col-arrow"></span>
              <span class="col-target">归入标准模型</span>
              <span class="col-action">操作</span>
            </div>
            <div
              v-for="(target, raw) in customModelMappings"
              :key="raw"
              class="m26-mapping-row"
            >
              <span class="col-source font-mono">{{ raw }}</span>
              <span class="col-arrow">➔</span>
              <span class="col-target font-mono font-semibold text-indigo-600">{{ target }}</span>
              <span class="col-action">
                <button
                  class="m26-btn-icon danger"
                  @click="emit('remove-mapping', String(raw))"
                  title="删除此条归类映射规则"
                >
                  <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor">
                    <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6" />
                  </svg>
                </button>
              </span>
            </div>
          </div>
        </div>
      </div>
      <div class="m26-pref-row" v-else>
        <div class="text-xs text-slate-400 py-2">
          暂无自定义归类规则。系统当前按内置智能规约算法聚合。遇到未识别的特殊名称可在上方添加映射，或在【模型】卡片上直接点击“归并”。
        </div>
      </div>
    </div>
  </div>

  <div class="m26-pref-group">
    <div class="m26-pref-group-header">
      <div class="m26-pref-group-icon icon-version">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="9" />
          <line x1="12" y1="11" x2="12" y2="16" />
          <circle cx="12" cy="8" r="0.5" fill="currentColor" />
        </svg>
      </div>
      <div class="m26-pref-group-titles">
        <h3 class="m26-pref-title">关于 HeiGate</h3>
        <span class="m26-pref-subtitle">应用构建信息</span>
      </div>
    </div>
    <div class="m26-pref-card">
      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <span class="label">当前版本</span>
        </div>
        <div class="m26-pref-ctrl">
          <span class="m26-db-chip font-mono">v{{ appVersion }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

