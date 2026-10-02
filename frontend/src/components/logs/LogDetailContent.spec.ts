// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import LogDetailContent from './LogDetailContent.vue'
import type { RequestLog } from '@/services/gatewayApi'

const log: RequestLog = {
  id: 12,
  created_at: '2026-10-02T10:00:00Z',
  model: 'gpt-4o',
  upstream_model: 'gpt-4o-mini',
  channel_id: 7,
  channel_name: 'Primary',
  endpoint: 'https://api.example.test/v1',
  status_code: 200,
  latency_ms: 1500,
  ttft_ms: 300,
  prompt_tokens: 10,
  completion_tokens: 20,
  total_tokens: 30,
  is_stream: true,
  is_failover: true,
  failover_from: 'Backup',
  error_message: '',
  request_path: '/v1/messages',
  finish_reason: 'stop',
}

const diagnostic = {
  badge: 'amber' as const,
  codeText: '故障轮转 200',
  title: '前序渠道故障，已完成轮转',
  message: '请求已由备用渠道完成。',
}

describe('LogDetailContent', () => {
  it('renders the diagnostic, failover trace and usage summary', () => {
    const wrapper = mount(LogDetailContent, { props: { log, diagnostic } })

    expect(wrapper.text()).toContain('前序渠道故障，已完成轮转')
    expect(wrapper.text()).toContain('Backup')
    expect(wrapper.text()).toContain('Primary')
    expect(wrapper.text()).toContain('20')
    expect(wrapper.text()).toContain('1.50s')
  })

  it('emits copy actions with the relevant payload', async () => {
    const wrapper = mount(LogDetailContent, { props: { log, diagnostic } })

    const copyJsonButton = wrapper
      .findAll('button')
      .find((button) => button.text() === '复制完整 JSON')
    await copyJsonButton?.trigger('click')

    expect(wrapper.emitted('copy-log-json')).toHaveLength(1)
    expect(wrapper.emitted('copy-log-json')?.[0]).toEqual([log])
  })
})
