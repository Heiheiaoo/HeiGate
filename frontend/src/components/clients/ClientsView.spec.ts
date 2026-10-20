import { beforeEach, afterEach, describe, it, expect, vi } from 'vitest'
import { mount, flushPromises, enableAutoUnmount } from '@vue/test-utils'
import ClientsView from './ClientsView.vue'
import type { Channel } from '@/components/channels/types'

import { getClientConfigs, getGatewayConnection, updateClientConfig } from '@/services/gatewayApi'

vi.mock('@/services/gatewayApi', () => ({
  getClientConfigs: vi.fn(),
  getGatewayConnection: vi.fn(),
  updateClientConfig: vi.fn(),
}))

enableAutoUnmount(afterEach)
beforeEach(() => {
  vi.resetAllMocks()
  const draft = new Map<string, string>()
  vi.stubGlobal('localStorage', {
    getItem: vi.fn((key: string) => draft.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => { draft.set(key, value) }),
  })
  vi.mocked(getClientConfigs).mockResolvedValue({})
  vi.mocked(getGatewayConnection).mockResolvedValue({
    base_url: 'http://127.0.0.1:28888/v1',
    api_key: 'gateway-test-key',
  })
  vi.mocked(updateClientConfig).mockResolvedValue({ ok: true })
})
afterEach(() => { vi.unstubAllGlobals() })

const mockChannels: Channel[] = [
  {
    name: 'Anthropic Official',
    url: 'https://api.anthropic.com',
    protocol: 'anthropic',
    status: 'healthy',
    latency: 320,
    ttft: 180,
    models: 3,
    lastCheck: '刚刚',
    accent: '#f97316',
    enabled: true,
    apiKey: 'sk-ant-test-key-12345678',
    primaryModel: 'claude-3-7-sonnet-latest',
    modelNames: ['claude-3-7-sonnet-latest', 'claude-3-5-sonnet-20241022', 'claude-3-5-haiku-latest'],
  },
  {
    name: 'OpenAI Direct',
    url: 'https://api.openai.com/v1',
    protocol: 'openai-compatible',
    status: 'healthy',
    latency: 210,
    ttft: 150,
    models: 2,
    lastCheck: '刚刚',
    accent: '#10b981',
    enabled: true,
    apiKey: 'sk-proj-test-key-87654321',
    primaryModel: 'gpt-4o',
    modelNames: ['gpt-4o', 'o3-mini'],
  },
]

describe('ClientsView.vue', () => {
  it('omits the redundant Hermes introduction card', () => {
    const wrapper = mount(ClientsView, { props: { channels: mockChannels } })
    expect(wrapper.find('.m26-client-hero-card').exists()).toBe(false)
  })

  it('renders Hermes gateway configuration with allowed models and dispatch strategy', async () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
      },
    })
    await flushPromises()

    expect(wrapper.find('.m26-client-hero-card').exists()).toBe(false)
    // No useless direct mode toggle rendered for Hermes
    expect(wrapper.find('.compact-hermes').exists()).toBe(false)
    // Shows the core panels and controls
    expect(wrapper.text()).toContain('供应商')
    expect(wrapper.text()).toContain('调度模型池')
    expect(wrapper.text().toUpperCase()).toContain('是否启用AUTO')
    expect(wrapper.text()).not.toContain('保存并重启')
    expect(wrapper.text()).toContain('http://127.0.0.1:28888/v1')
    expect(wrapper.text()).toContain('gateway-test-key')
    // Shows models from current channel
    expect(wrapper.text()).toContain('claude-3-7-sonnet')

    // Test channel chips directly visible and clickable in single row
    const channelCards = wrapper.findAll('.m26-channel-chip')
    expect(channelCards.length).toBeGreaterThan(0)

    // Test select all action link
    const selectAllLinks = wrapper.findAll('.m26-action-link')
    expect(selectAllLinks.length).toBeGreaterThan(0)
    await selectAllLinks[0].trigger('click')

    // Test clicking a channel chip
    await channelCards[0].trigger('click')

    // Test strategy custom dropdown trigger and options selection
    const strategyTrigger = wrapper.find('.m26-strategy-trigger')
    expect(strategyTrigger.exists()).toBe(true)
    expect(strategyTrigger.text()).toContain('智能综合质量优先')
    expect(wrapper.find('.m26-strategy-options-menu').exists()).toBe(false)

    // Open dropdown
    await strategyTrigger.trigger('click')
    expect(wrapper.find('.m26-strategy-options-menu').exists()).toBe(true)
    const options = wrapper.findAll('.m26-strategy-option-item')
    expect(options.length).toBe(3)

    // Select second option (lowest latency)
    await options[1].trigger('click')
    expect(wrapper.find('.m26-strategy-options-menu').exists()).toBe(false)
    expect(strategyTrigger.text()).toContain('最低首字延迟')

    // Test help icon shows Auto role and mode differences
    const helpBtn = wrapper.find('.m26-hint-icon-btn')
    expect(helpBtn.exists()).toBe(true)
    await helpBtn.trigger('click')
    const helpPopover = wrapper.find('.m26-auto-help-popover')
    expect(helpPopover.exists()).toBe(true)
    expect(helpPopover.text()).toContain('Auto 核心作用')
    expect(helpPopover.text()).toContain('各个模式区别')
    expect(helpPopover.text()).toContain('轮询负载均衡')
  })

})
