<script setup lang="ts">
defineProps<{
  hideDockIcon: boolean
  supported: boolean
}>()

const emit = defineEmits<{
  'update:hideDockIcon': [value: boolean]
}>()
</script>

<template>
  <div v-if="supported" class="m26-pref-group">
    <div class="m26-pref-group-header">
      <div class="m26-pref-group-icon icon-desktop">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
          <line x1="8" y1="21" x2="16" y2="21" />
          <line x1="12" y1="17" x2="12" y2="21" />
        </svg>
      </div>
      <div class="m26-pref-group-titles">
        <h3 class="m26-pref-title">桌面偏好与系统设置</h3>
        <span class="m26-pref-subtitle">管理桌面应用的外观呈现、程序坞与菜单栏常驻运行行为</span>
      </div>
    </div>

    <div class="m26-pref-card">
      <div class="m26-pref-row">
        <div class="m26-pref-meta">
          <div class="flex items-center gap-2">
            <span class="label">是否隐藏Dock图标（仅 macOS）</span>
            <span class="m26-db-chip">macOS</span>
          </div>
          <span class="desc">开启后隐藏 macOS 程序坞中的应用图标，应用常驻在顶部菜单栏托盘中运行；关闭后恢复在程序坞中显示</span>
        </div>
        <div class="m26-pref-ctrl">
          <label class="m26-switch-label" title="切换是否隐藏 Dock 图标">
            <input
              type="checkbox"
              class="m26-switch-input"
              :checked="hideDockIcon"
              @change="emit('update:hideDockIcon', ($event.target as HTMLInputElement).checked)"
            />
            <span class="m26-switch-slider"></span>
          </label>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.m26-pref-group-icon.icon-desktop {
  background: rgba(139, 92, 246, 0.1);
  color: #8b5cf6;
}

/* Switch styling */
.m26-switch-label {
  position: relative;
  display: inline-block;
  width: 28px;
  height: 16px;
  cursor: pointer;
}

.m26-switch-input {
  opacity: 0;
  width: 0;
  height: 0;
}

.m26-switch-slider {
  position: absolute;
  inset: 0;
  background-color: #cbd5e1;
  border-radius: 16px;
  transition: 0.15s;
}

:root.dark .m26-switch-slider {
  background-color: #475569;
}

.m26-switch-slider::before {
  position: absolute;
  content: "";
  height: 12px;
  width: 12px;
  left: 2px;
  bottom: 2px;
  background-color: white;
  border-radius: 50%;
  transition: 0.15s;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.15);
}

.m26-switch-input:checked + .m26-switch-slider {
  background-color: #10b981;
}

.m26-switch-input:checked + .m26-switch-slider::before {
  transform: translateX(12px);
}
</style>
