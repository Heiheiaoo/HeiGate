// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import StorageSettings from './StorageSettings.vue'

const props = {
  systemInfo: { data_dir: '/test', db_path: '/test/gateway.db', db_size_bytes: 2048, logs_count: 42, log_retention_days: 7, log_max_count: 5000, routing_strategy: 'latency' as const },
  fallbackLogsCount: 12,
  isPruningLogs: false,
  logRetentionDays: 7,
  logMaxCount: 5000,
}

describe('StorageSettings', () => {
  it('renders database metadata and emits path and prune actions', async () => {
    const wrapper = mount(StorageSettings, { props })
    expect(wrapper.text()).toContain('2.0 KB')
    expect(wrapper.text()).toContain('42 条流水')
    await wrapper.get('[title="复制数据库绝对路径"]').trigger('click')
    expect(wrapper.emitted('copy-path')).toEqual([['/test/gateway.db']])
    await wrapper.get('[title="按当前策略立即执行一次修剪清理"]').trigger('click')
    expect(wrapper.emitted('prune-logs')).toEqual([[]])
    await wrapper.setProps({ isPruningLogs: true })
    const prune = wrapper.get<HTMLButtonElement>('[title="按当前策略立即执行一次修剪清理"]')
    expect(prune.element.disabled).toBe(true)
    expect(prune.text()).toBe('清理中...')
    await prune.trigger('click')
    expect(wrapper.emitted('prune-logs')).toHaveLength(1)
  })

  it('hides unavailable database controls but retains configuration actions', () => {
    const wrapper = mount(StorageSettings, { props: { ...props, systemInfo: null } })
    expect(wrapper.find('[title="复制数据库绝对路径"]').exists()).toBe(false)
    expect(wrapper.find('[title="按当前策略立即执行一次修剪清理"]').exists()).toBe(false)
    expect(wrapper.findAll('select')).toHaveLength(2)
    expect(wrapper.text()).toContain('导入配置')
    expect(wrapper.emitted()).toEqual({})
  })

  it('emits numeric policy values, including unlimited, without mutating props', async () => {
    const wrapper = mount(StorageSettings, { props })
    const [retention, count] = wrapper.findAll('select')
    expect(retention.element.value).toBe('7')
    expect(count.element.value).toBe('5000')
    for (const value of [1, 3, 7, 14, 30, 0]) await retention.setValue(String(value))
    for (const value of [1000, 3000, 5000, 10000, 50000, 0]) await count.setValue(String(value))
    expect(wrapper.emitted('set-retention')).toEqual([1, 3, 7, 14, 30, 0].map(value => [value]))
    expect(wrapper.emitted('set-max-count')).toEqual([1000, 3000, 5000, 10000, 50000, 0].map(value => [value]))
    expect(wrapper.props('logRetentionDays')).toBe(7)
    expect(wrapper.props('logMaxCount')).toBe(5000)
  })

  it('only emits explicit backup, import and reset intents; mount and rerender do nothing', async () => {
    const wrapper = mount(StorageSettings, { props })
    await wrapper.setProps({ logRetentionDays: 30 })
    expect(wrapper.emitted()).toEqual({})
    for (const [title, event] of [
      ['导出当前所有渠道与配置', 'export-config'],
      ['导入已有配置文件', 'import-config'],
      ['清空本机保存的所有通道配置', 'clear-workspace'],
    ]) {
      await wrapper.get(`[title="${title}"]`).trigger('click')
      expect(wrapper.emitted(event)).toEqual([[]])
    }
  })
})
