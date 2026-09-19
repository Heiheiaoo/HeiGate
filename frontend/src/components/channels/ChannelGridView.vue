<script setup lang="ts">
import type { Channel } from './types'
import ChannelCard from './ChannelCard.vue'

defineProps<{
  channels: Channel[]
  selectedChannel?: Channel | null
  searchQuery?: string
}>()

const emit = defineEmits<{
  (e: 'select', c: Channel): void
  (e: 'toggle', c: Channel): void
  (e: 'probe', c: Channel): void
  (e: 'edit', c: Channel): void
  (e: 'duplicate', c: Channel): void
  (e: 'remove', c: Channel): void
  (e: 'open-models', c: Channel): void
  (e: 'open-diagnostic', c: Channel): void
  (e: 'open-inject', c: Channel): void
  (e: 'add-channel'): void
  (e: 'clear-search'): void
}>()
</script>

<template>
  <div v-if="channels.length > 0" class="m26-channel-grid">
    <ChannelCard
      v-for="c in channels"
      :key="c.gatewayId ?? c.name"
      :channel="c"
      :is-selected="selectedChannel?.gatewayId ? selectedChannel.gatewayId === c.gatewayId : selectedChannel?.name === c.name"
      @select="emit('select', $event)"
      @toggle="emit('toggle', $event)"
      @probe="emit('probe', $event)"
      @edit="emit('edit', $event)"
      @duplicate="emit('duplicate', $event)"
      @remove="emit('remove', $event)"
      @open-models="emit('open-models', $event)"
      @open-diagnostic="emit('open-diagnostic', $event)"
      @open-inject="emit('open-inject', $event)"
    />
  </div>

  <!-- Empty State -->
  <div v-else class="m26-ch-empty-card">
    <div class="empty-icon-box">
      <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" stroke-width="1.6">
        <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
        <line x1="8" y1="21" x2="16" y2="21" />
        <line x1="12" y1="17" x2="12" y2="21" />
      </svg>
    </div>
    <div class="empty-text-box">
      <h5 v-if="searchQuery" class="empty-title">未找到匹配 “{{ searchQuery }}” 的渠道</h5>
      <h5 v-else class="empty-title">暂无已配置的供应商渠道</h5>
      <p class="empty-subtitle">
        {{ searchQuery ? '请检查搜索关键词或重置筛选条件' : '添加官方 API、中转商或本地 Ollama 渠道，开启统一本地网关' }}
      </p>
    </div>
    <div class="empty-actions">
      <button
        v-if="searchQuery"
        class="m26-glass-btn text-xs"
        @click="emit('clear-search')"
      >
        清空筛选
      </button>
      <button
        v-else
        class="m26-action-btn primary text-xs"
        @click="emit('add-channel')"
      >
        添加第一个渠道
      </button>
    </div>
  </div>
</template>

<style scoped>
.m26-channel-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(330px, 1fr));
  gap: 16px;
}

@media (min-width: 1400px) {
  .m26-channel-grid {
    grid-template-columns: repeat(auto-fill, minmax(350px, 1fr));
    gap: 18px;
  }
}

.m26-ch-empty-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px 24px;
  background: rgba(255, 255, 255, 0.6);
  border: 1px dashed rgba(0, 0, 0, 0.12);
  border-radius: 12px;
  text-align: center;
}

.empty-icon-box {
  color: #94a3b8;
  margin-bottom: 12px;
}

.empty-title {
  margin: 0 0 4px;
  font-size: 14px;
  font-weight: 600;
  color: #1e293b;
}

.empty-subtitle {
  margin: 0 0 16px;
  font-size: 12px;
  color: #64748b;
  max-width: 320px;
}
</style>
