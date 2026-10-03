// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import LogsView from './LogsView.vue'

const logs = [{ id: 1, created_at: '2026-10-03T10:00:00Z', status_code: 200, model: 'gpt-5', channel_id: 1, channel_name: 'Primary', latency_ms: 1200, ttft_ms: 300, prompt_tokens: 10, completion_tokens: 20, is_stream: true, is_failover: false, failover_from: '', upstream_model: '', error_message: '' }]

describe('LogsView', () => {
  it('renders log rows and emits row actions', async () => {
    const wrapper = mount(LogsView, { props: { logs, totalLogs: 1, search: '', streamConnected: true, autoScroll: true, clearing: false } })
    expect(wrapper.text()).toContain('gpt-5')
    expect(wrapper.text()).toContain('1.20s')
    await wrapper.get('tbody tr').trigger('click')
    expect(wrapper.emitted('open-detail')).toHaveLength(1)
  })

  it('emits search, scroll, and clear actions', async () => {
    const wrapper = mount(LogsView, { props: { logs: [], totalLogs: 0, search: 'error', streamConnected: false, autoScroll: true, clearing: false } })
    await wrapper.get('input').setValue('timeout')
    await wrapper.findAll('button.m26-glass-btn')[0].trigger('click')
    await wrapper.get('button.danger').trigger('click')
    expect(wrapper.emitted('update:search')).toContainEqual(['timeout'])
    expect(wrapper.emitted('update:auto-scroll')).toContainEqual([false])
    expect(wrapper.emitted('clear')).toHaveLength(1)
  })
})
