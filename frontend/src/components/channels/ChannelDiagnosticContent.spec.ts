// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelDiagnosticContent from './ChannelDiagnosticContent.vue'
import type { Channel } from './types'

const baseChannel: Channel = {
  name: 'Primary',
  url: 'https://api.example.test/v1',
  protocol: 'openai-compatible',
  status: 'offline',
  latency: 1200,
  ttft: null,
  models: 1,
  lastCheck: '刚刚',
  accent: '#3b82f6',
  enabled: true,
  primaryModel: 'gpt-4o',
  modelNames: ['gpt-4o'],
  lastError: 'HTTP 401 unauthorized',
}

function mountDiagnostic(lastError: string | undefined) {
  return mount(ChannelDiagnosticContent, {
    props: {
      channel: { ...baseChannel, lastError },
    },
  })
}

describe('ChannelDiagnosticContent', () => {
  it.each([
    ['HTTP 401 unauthorized', 'HTTP 401', '检查 API 密钥'],
    ['HTTP 404 model route not found', 'HTTP 404', '核对探活主模型'],
    ['HTTP 429 rate limited', 'HTTP 429', '检查账号额度'],
    ['request timeout exceeded', 'TIMEOUT', '检查本地网络与代理'],
  ])('renders %s diagnostics and advice', (lastError, code, advice) => {
    const wrapper = mountDiagnostic(lastError)

    expect(wrapper.text()).toContain(code)
    expect(wrapper.text()).toContain(advice)
  })

  it('renders a default summary when no error is available', () => {
    const wrapper = mountDiagnostic(undefined)

    expect(wrapper.text()).toContain('未知异常')
    expect(wrapper.text()).toContain('查看原始报错信息')
    expect(wrapper.text()).toContain('无原始回包')
  })

  it('emits the raw error when copying diagnostics', async () => {
    const wrapper = mountDiagnostic(baseChannel.lastError)
    const button = wrapper.findAll('button').find((item) => item.text().includes('复制完整报错'))

    await button?.trigger('click')

    expect(wrapper.emitted('copy-raw')).toEqual([[baseChannel.lastError]])
  })
})
