import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ClientsView from './ClientsView.vue'
import type { Channel } from '@/components/channels/types'

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
  it('renders all five platform cards in Hermes, Codex, Claude Desktop, Claude Code, 通用 order', () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
        gatewayKey: 'heigate-secret',
      },
    })

    const names = wrapper.findAll('.m26-client-tab-name').map((el) => el.text())
    expect(names).toEqual(['Hermes', 'Codex', 'Claude Desktop', 'Claude Code', '通用'])
  })

  it('renders universal gateway access settings and routing settings when selecting generic platform', async () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
        gatewayBaseUrl: 'http://127.0.0.1:28888/v1',
        gatewayKey: 'heigate-secret-123',
        routingStrategy: 'smart_quality',
      },
    })

    const tabCards = wrapper.findAll('.m26-client-tab-card')
    // Click 5th card: 通用
    await tabCards[4].trigger('click')

    expect(wrapper.find('.m26-hero-title').text()).toBe('通用')
    expect(wrapper.text()).toContain('常见客户端快速接入指引')
    expect(wrapper.text()).toContain('Cursor / Windsurf')
    expect(wrapper.text()).toContain('Cline / Roo Code / Continue')
  })

  it('switches platform when clicking a deck item', async () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
      },
    })

    // Default active is Hermes (index 0)
    expect(wrapper.find('.m26-client-tab-card.hermes').classes()).toContain('active')

    // Click on Codex (index 1)
    const tabCards = wrapper.findAll('.m26-client-tab-card')
    await tabCards[1].trigger('click')

    expect(wrapper.find('.m26-hero-title').text()).toBe('Codex')

    // Click on Claude Desktop (index 2)
    await tabCards[2].trigger('click')
    expect(wrapper.find('.m26-hero-title').text()).toBe('Claude Desktop')
  })

  it('renders Hermes dedicated gateway configuration with allowed models and dispatch strategy', async () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
      },
    })

    // Hermes is active by default
    expect(wrapper.find('.m26-client-tab-card.hermes').classes()).toContain('active')
    // No useless direct mode toggle rendered for Hermes
    expect(wrapper.find('.compact-hermes').exists()).toBe(false)
    // Shows the core panels and controls
    expect(wrapper.text()).toContain('供应商')
    expect(wrapper.text()).toContain('调度模型池')
    expect(wrapper.text().toUpperCase()).toContain('是否启用AUTO')
    expect(wrapper.text()).toContain('保存并重启')
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

  it('filters compatible channels by protocol for Claude vs Codex', async () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
      },
    })

    const tabCards = wrapper.findAll('.m26-client-tab-card')

    // Switch to Claude Desktop (index 2): select should only contain Anthropic Official
    await tabCards[2].trigger('click')
    const claudeSelect = wrapper.find('.m26-select')
    const claudeOptions = claudeSelect.findAll('option').map((o) => o.text())
    expect(claudeOptions.some((txt) => txt.includes('Anthropic Official'))).toBe(true)
    expect(claudeOptions.some((txt) => txt.includes('OpenAI Direct'))).toBe(false)

    // Switch to Codex (index 1)
    await tabCards[1].trigger('click')

    const codexSelect = wrapper.find('.m26-select')
    const codexOptions = codexSelect.findAll('option').map((o) => o.text())
    expect(codexOptions.some((txt) => txt.includes('OpenAI Direct'))).toBe(true)
    expect(codexOptions.some((txt) => txt.includes('Anthropic Official'))).toBe(false)
  })

  it('emits save event with correct payload on clicking save buttons', async () => {
    const wrapper = mount(ClientsView, {
      props: {
        channels: mockChannels,
      },
    })

    // Switch to Claude Desktop (index 2)
    const tabCards = wrapper.findAll('.m26-client-tab-card')
    await tabCards[2].trigger('click')

    // Click "保存配置" (without launch)
    await wrapper.find('.m26-btn-ghost').trigger('click')
    expect(wrapper.emitted('save')).toBeTruthy()
    expect(wrapper.emitted('save')![0][0]).toMatchObject({
      platformId: 'claude-desktop',
      mode: 'direct',
      launchAfterSave: false,
    })

    // Click "保存并启动" (with launch)
    await wrapper.find('.m26-btn-primary').trigger('click')
    expect(wrapper.emitted('save')![1][0]).toMatchObject({
      platformId: 'claude-desktop',
      mode: 'direct',
      launchAfterSave: true,
    })
  })
})
