import { beforeEach, afterEach, describe, it, expect, vi } from 'vitest'
import { mount, flushPromises, enableAutoUnmount } from '@vue/test-utils'
import ClientsView from './ClientsView.vue'
import type { Channel } from '@/components/channels/types'

import { getClientConfigs, updateClientConfig, probeClientConnection, applyCodexConfig, getCodexConfigStatus } from '@/services/gatewayApi'

vi.mock('@/services/gatewayApi', () => ({
  getClientConfigs: vi.fn(),
  updateClientConfig: vi.fn(),
  probeClientConnection: vi.fn(),
  applyCodexConfig: vi.fn(),
  getCodexConfigStatus: vi.fn(),
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
  vi.mocked(updateClientConfig).mockResolvedValue({ ok: true })
  vi.mocked(applyCodexConfig).mockResolvedValue({ ok: true, message: '已配置并重启 Codex' })
  vi.mocked(getCodexConfigStatus).mockResolvedValue({ path: '~/.codex/config.toml', exists: true, managed: false, modified: false, backup_exists: false })
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

  it('saves direct settings as a local draft without claiming a client was launched', async () => {
    const wrapper = mount(ClientsView, { props: { channels: mockChannels } })
    await wrapper.findAll('.m26-client-tab-card')[2].trigger('click')
    expect(wrapper.find('.m26-hero-actions').text()).toContain('保存直连草稿')
    expect(wrapper.find('.m26-hero-actions').text()).not.toContain('启动')
    await wrapper.find('.m26-hero-actions button').trigger('click')
    await flushPromises()
    expect(updateClientConfig).not.toHaveBeenCalled()
    const saved = JSON.parse(localStorage.getItem('heigate_client_platforms_draft')!)
    expect(saved['claude-desktop'].direct.endpoint).toBe('https://api.anthropic.com')
    expect(saved['claude-desktop'].direct.apiKey).toBeUndefined()
    expect(localStorage.getItem('heigate_client_platforms_draft')).not.toContain(mockChannels[0].apiKey)
    expect(wrapper.find('[role="status"]').text()).toContain('直连草稿已保存在此设备')
    expect(wrapper.text()).not.toContain('已检测到 macOS 本机配置文件')
  })

  it('reports direct Codex protocol failures before attempting a restart', async () => {
    const wrapper = mount(ClientsView, { props: { channels: mockChannels } })
    await wrapper.findAll('.m26-client-tab-card')[1].trigger('click')
    await wrapper.find('.m26-hero-actions button').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="status"]').text()).toContain('不是 Responses 协议')
    expect(wrapper.find('[role="status"]').classes()).toContain('text-red-600')
  })

  it('allows direct Codex setup when a Chat channel is also confirmed to support Responses', async () => {
    const dualProtocolChannels = mockChannels.map(channel => channel.name === 'OpenAI Direct'
      ? { ...channel, gatewayId: 42, supportsResponses: true }
      : channel)
    const wrapper = mount(ClientsView, { props: { channels: dualProtocolChannels } })
    await wrapper.findAll('.m26-client-tab-card')[1].trigger('click')

    expect(wrapper.text()).toContain('Responses 可用')
    await wrapper.find('.m26-hero-actions button').trigger('click')
    await flushPromises()

    expect(applyCodexConfig).toHaveBeenCalledWith(expect.objectContaining({
      mode: 'direct',
      channel_id: 42,
      models: expect.arrayContaining(['gpt-4o', 'o3-mini']),
      restart: true,
    }))
    expect(wrapper.find('[role="status"]').text()).toContain('已配置并重启 Codex')
  })

  it('waits for gateway save and shows API failures instead of a success toast', async () => {
    const wrapper = mount(ClientsView, { props: { channels: mockChannels } })
    await wrapper.findAll('.m26-client-tab-card')[1].trigger('click')
    await wrapper.findAll('.m26-mode-btn')[1].trigger('click')
    vi.mocked(updateClientConfig).mockResolvedValueOnce({ ok: false, message: '数据库写入失败' })
    await wrapper.find('.m26-hero-actions button').trigger('click')
    await flushPromises()
    vi.mocked(applyCodexConfig).mockResolvedValueOnce({ ok: false, message: '数据库写入失败' })
    await wrapper.find('.m26-hero-actions button').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="status"]').text()).toBe('数据库写入失败')
    expect(wrapper.text()).not.toContain('正在启动 Codex')
    expect(updateClientConfig).toHaveBeenCalledWith('codex', expect.objectContaining({ strategy: 'smart_quality' }))
    await wrapper.find('.m26-hero-actions button').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="status"]').text()).toContain('已配置并重启 Codex')
  })

  it('shows real probe latency and upstream or network errors', async () => {
    const wrapper = mount(ClientsView, { props: { channels: mockChannels } })
    await wrapper.findAll('.m26-client-tab-card')[1].trigger('click')
    let resolveProbe!: (result: { model: string; status: string; latencyMs: number }) => void
    vi.mocked(probeClientConnection).mockImplementationOnce(() => new Promise(resolve => { resolveProbe = resolve }))
    await wrapper.find('.m26-btn-test').trigger('click')
    expect(wrapper.find('.m26-btn-test').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).not.toContain('168ms')
    expect(probeClientConnection).toHaveBeenCalledWith(expect.objectContaining({
      endpoint: mockChannels[1].url, api_key: mockChannels[1].apiKey, primary_model: 'gpt-4o',
    }))
    resolveProbe({ model: 'gpt-4o', status: 'operational', latencyMs: 423 })
    await flushPromises()
    expect(wrapper.find('.m26-test-result-msg').text()).toContain('423ms')
    vi.mocked(probeClientConnection).mockResolvedValueOnce({ model: 'gpt-4o', status: 'error', message: 'upstream HTTP 401' })
    await wrapper.find('.m26-btn-test').trigger('click')
    await flushPromises()
    expect(wrapper.find('.m26-test-result-msg').classes()).toContain('failed')
    expect(wrapper.find('.m26-test-result-msg').text()).toContain('401')
    vi.mocked(probeClientConnection).mockRejectedValueOnce(new Error('connection refused'))
    await wrapper.find('.m26-btn-test').trigger('click')
    await flushPromises()
    expect(wrapper.find('.m26-test-result-msg').text()).toContain('connection refused')
  })

  it('shows Hermes action failure without claiming restart success', async () => {
    const wrapper = mount(ClientsView, { props: { channels: mockChannels } })
    vi.mocked(updateClientConfig).mockResolvedValueOnce({ ok: false, message: 'Hermes 重启失败' })
    await wrapper.find('.m26-btn-save-restart').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Hermes 重启失败')
    expect(wrapper.text()).not.toContain('成功重启 Hermes')
  })

  it('uses the same client-specific gateway URL in copied and displayed commands', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    const wrapper = mount(ClientsView, { props: { channels: mockChannels, gatewayBaseUrl: 'http://127.0.0.1:28889/v1', gatewayKey: 'gateway-key' } })
    await wrapper.findAll('.m26-client-tab-card')[1].trigger('click')
    await wrapper.findAll('.m26-mode-btn')[1].trigger('click')
    await wrapper.find('.m26-cli-env-card button').trigger('click')
    expect(wrapper.find('.m26-cli-code').text()).toContain('/codex/v1')
    expect(writeText).toHaveBeenCalledWith(wrapper.find('.m26-cli-code').text())
    await wrapper.findAll('.m26-mode-btn')[0].trigger('click')
    expect(wrapper.find('.m26-cli-code').text()).toContain(mockChannels[1].url)
    expect(wrapper.find('.m26-cli-code').text()).not.toContain('gateway-key')
  })
})
