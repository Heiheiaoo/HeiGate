import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { enableAutoUnmount, flushPromises, shallowMount } from '@vue/test-utils'
import GatewayView from './GatewayView.vue'
import ChannelGridView from '@/components/channels/ChannelGridView.vue'
import ChannelToolbar from '@/components/channels/ChannelToolbar.vue'
import ChannelModelLatencyContent from '@/components/channels/ChannelModelLatencyContent.vue'
import ChannelModelManager from '@/components/channels/ChannelModelManager.vue'
import ChannelKeyInputs from '@/components/channels/ChannelKeyInputs.vue'
import ChannelInjectionContent from '@/components/channels/ChannelInjectionContent.vue'
import LogDetailContent from '@/components/logs/LogDetailContent.vue'
import LogsView from '@/components/logs/LogsView.vue'
import StorageSettings from '@/components/settings/StorageSettings.vue'
import DesktopGeneralSettings from '@/components/settings/DesktopGeneralSettings.vue'
import { listChannels, updateChannel, duplicateChannel, deleteChannel, updateSettings, getSystemInfo, detectChannelProtocol, fetchUpstreamModels, probeChannelDetails, injectionCheck, subscribeLogs, type ApiChannel, type SystemInfo } from '@/services/gatewayApi'
import { writeValue } from '@/services/storage'
import { _resetDetectedProtocolsCacheForTest, getChannelDetectedProtocols, saveChannelDetectedProtocols } from '@/services/channelProtocols'

vi.mock('@/services/gatewayApi', async (importOriginal) => ({
  ...await importOriginal<typeof import('@/services/gatewayApi')>(),
  listChannels: vi.fn(),
  updateChannel: vi.fn(),
  duplicateChannel: vi.fn(),
  deleteChannel: vi.fn(),
  detectChannelProtocol: vi.fn(),
  fetchUpstreamModels: vi.fn().mockResolvedValue({ ok: true, models: ['fetched-model'] }),
  probeChannelDetails: vi.fn(),
  injectionCheck: vi.fn(),
  updateSettings: vi.fn().mockResolvedValue(false),
  getCustomModelMappings: vi.fn().mockResolvedValue({}),
  getSystemInfo: vi.fn().mockResolvedValue(null),
  listLogs: vi.fn().mockResolvedValue([]),
  subscribeLogs: vi.fn(() => vi.fn()),
}))
vi.mock('@/services/storage', () => ({
  readValue: vi.fn().mockResolvedValue(null),
  writeValue: vi.fn(),
  clearValue: vi.fn(),
}))
vi.mock('@/services/health', () => ({
  checkGatewayHealth: vi.fn().mockResolvedValue({ ok: true, latency: 1, url: '', version: 'test' }),
}))

enableAutoUnmount(afterEach)
beforeEach(() => {
  vi.clearAllMocks()
  const storage = new Map([['heigate_active_section', 'channels']])
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => { storage.set(key, value) },
    removeItem: (key: string) => { storage.delete(key) },
  })
  _resetDetectedProtocolsCacheForTest()
  vi.mocked(listChannels).mockResolvedValue([channel()])
  vi.mocked(updateChannel).mockResolvedValue(null)
  vi.mocked(getSystemInfo).mockResolvedValue(null)
  vi.mocked(fetchUpstreamModels).mockResolvedValue({ ok: true, models: ['fetched-model'] })
  vi.mocked(injectionCheck).mockResolvedValue(null)
})
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers() })

function channel(): ApiChannel {
  return {
    id: 1, name: 'Supplier', provider: 'openai-compatible', apiMode: 'chat_completions',
    url: 'https://example.test', primaryModel: 'model', extraModels: [], enabled: true,
    status: 'healthy', latency: 1, ttft: 1, pingLatency: null, lastError: '', lastCheck: '',
    failureCount: 0, circuitState: 'closed', cooldownUntil: null, injectionState: 'unknown',
    injectionTokens: null, injectionContent: '', injectionCheckedAt: null, modelHealth: [],
  }
}

async function refresh() {
  window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'refresh' }))
  await flushPromises()
}

describe('GatewayView channel persistence', () => {
  it.each(['channel', 'model'] as const)('ignores a backend-rejected %s probe without overwriting refreshed state', async action => {
    let finish!: (result: Awaited<ReturnType<typeof probeChannelDetails>>) => void
    vi.mocked(probeChannelDetails).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('open-models', grid.props('channels')[0])
    await flushPromises()
    const content = wrapper.findComponent(ChannelModelLatencyContent)
    if (action === 'channel') grid.vm.$emit('probe', grid.props('channels')[0])
    else content.vm.$emit('probe-model', 'model')
    await flushPromises()
    const checkedAt = new Date(Date.now() - 2000).toISOString()
    vi.mocked(listChannels).mockResolvedValue([{
      ...channel(), latency: 900, ttft: 900, lastCheck: checkedAt,
      modelHealth: [{ model: 'model', status: 'operational', latencyMs: 900, ttftMs: 900, error: '', checkedAt }],
    }])
    finish([{ model: 'model', status: 'error', message: 'rejected old-key error', accepted: false,
      latencyMs: null, ttftMs: null, checkedAt: new Date().toISOString() }])
    await flushPromises()
    expect(grid.props('channels')[0]).toMatchObject({ status: 'healthy', latency: 900, ttft: 900, probing: false })
    expect(content.props('rows')[0]).toMatchObject({ latencyDisplay: '0.90s', probing: false })
    expect(content.props('rows')[0].latencyTooltip).not.toContain('rejected old-key error')
  })

  it.each(['primary', 'secondary', 'batch', 'modal'] as const)('ignores backend-rejected %s injection diagnostics', async action => {
    const checkedAt = new Date(Date.now() - 2000).toISOString()
    vi.mocked(listChannels).mockResolvedValue([{
      ...channel(), extraModels: ['extra'], injectionState: 'clean', injectionTokens: 20,
      injectionCheckedAt: checkedAt,
    }])
    vi.mocked(injectionCheck).mockImplementation(async (_id, options) => [{
      model: typeof options === 'object' ? options.model || 'model' : 'model',
      state: 'detected', prompt_tokens: 999, content: 'old diagnostic',
      checked_at: new Date().toISOString(), accepted: false,
    }])
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('open-models', grid.props('channels')[0])
    await flushPromises()
    const content = wrapper.findComponent(ChannelModelLatencyContent)
    if (action === 'modal') {
      grid.vm.$emit('open-inject', grid.props('channels')[0])
      await flushPromises()
      wrapper.findComponent(ChannelInjectionContent).vm.$emit('check-one')
    } else if (action === 'batch') {
      content.vm.$emit('batch-action', 'injection')
      await flushPromises()
      await wrapper.find('.m26-confirm-backdrop .m26-action-btn.primary').trigger('click')
    } else content.vm.$emit('check-injection', action === 'primary' ? 'model' : 'extra')
    await flushPromises()
    expect(injectionCheck).toHaveBeenCalled()
    expect(grid.props('channels')[0]).toMatchObject({ injectionState: 'clean', injectionTokens: 20, injectionCheckedAt: checkedAt })
    const rows = content.props('rows')
    expect(rows.find(row => row.name === 'model')).toMatchObject({ injectionClass: 'clean', checkingInjection: false })
    expect(rows.find(row => row.name === 'extra')).toMatchObject({ injectionClass: 'idle', checkingInjection: false })
    if (action === 'modal') {
      const modal = wrapper.findComponent(ChannelInjectionContent)
      expect(modal.props('checking')).toBe(false)
      expect(modal.props('results')).toEqual([expect.objectContaining({ state: 'clean', prompt_tokens: 20 })])
    }
  })

  it('counts only accepted results in injection diagnostic feedback', async () => {
    vi.mocked(listChannels).mockResolvedValue([{ ...channel(), extraModels: ['extra'] }])
    vi.mocked(injectionCheck).mockResolvedValue([
      { model: 'model', state: 'clean', prompt_tokens: 20, checked_at: new Date().toISOString(), accepted: true },
      { model: 'extra', state: 'detected', prompt_tokens: 999, checked_at: new Date().toISOString(), accepted: false },
    ])
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('open-inject', grid.props('channels')[0])
    await flushPromises()
    const modal = wrapper.findComponent(ChannelInjectionContent)
    modal.vm.$emit('check-one')
    await flushPromises()
    expect(modal.props('results')).toHaveLength(1)
    expect(modal.props('results')[0].state).toBe('clean')
    expect(wrapper.find('.m26-toast-text').text()).toContain('1 个模型的输入统计均处于正常模板参考范围')
  })

  it.each(['endpoint', 'live failure'] as const)('does not apply an old model probe after a newer %s', async change => {
    let finish!: (result: Awaited<ReturnType<typeof probeChannelDetails>>) => void
    vi.mocked(probeChannelDetails).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('open-models', grid.props('channels')[0])
    await flushPromises()
    const content = wrapper.findComponent(ChannelModelLatencyContent)
    content.vm.$emit('probe-model', 'model')
    await flushPromises()
    const oldTime = new Date(Date.now() - 2000).toISOString()
    const newTime = new Date().toISOString()
    const updated = change === 'endpoint' ? { ...channel(), url: 'https://new.test', status: 'unknown' as const }
      : { ...channel(), status: 'offline' as const, lastCheck: newTime, lastError: 'new live failure', modelHealth: [{ model: 'model', status: 'error', latencyMs: null, ttftMs: null, error: 'new live failure', checkedAt: newTime }] }
    vi.mocked(listChannels).mockResolvedValue([updated])
    finish([{ model: 'model', status: 'operational', latencyMs: 20, ttftMs: 10, checkedAt: oldTime }])
    await flushPromises()
    expect(grid.props('channels')[0].url).toBe(updated.url)
    expect(grid.props('channels')[0].status).toBe(updated.status)
    expect(content.props('rows')[0].latencyTooltip).not.toContain('20ms')
    expect(content.props('rows')[0].probing).toBeFalsy()
    if (change === 'live failure') expect(content.props('rows')[0].latencyTooltip).toContain('new live failure')
  })

  it('ignores an older channel refresh completing after a newer refresh', async () => {
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    let finishOld!: (result: ApiChannel[]) => void
    vi.mocked(listChannels).mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve }))
    window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'refresh' }))
    await flushPromises()
    vi.mocked(listChannels).mockResolvedValue([{ ...channel(), url: 'https://new.test', status: 'offline' }])
    await refresh()
    finishOld([channel()])
    await flushPromises()
    expect(wrapper.findComponent(ChannelGridView).props('channels')[0].url).toBe('https://new.test')
    expect(wrapper.findComponent(ChannelGridView).props('channels')[0].status).toBe('offline')
  })

  it('drops stale model timing cache when the endpoint changes', async () => {
    vi.mocked(listChannels).mockResolvedValue([{ ...channel(), modelHealth: [{ model: 'model', status: 'operational', latencyMs: 900, ttftMs: 900, error: '', checkedAt: new Date().toISOString() }] }])
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('open-models', grid.props('channels')[0])
    await flushPromises()
    const content = wrapper.findComponent(ChannelModelLatencyContent)
    expect(content.props('rows')[0].latencyDisplay).toBe('0.90s')
    vi.mocked(listChannels).mockResolvedValue([{ ...channel(), url: 'https://new.test', status: 'unknown', latency: null, ttft: null }])
    await refresh()
    expect(content.props('rows')[0].latencyDisplay).not.toBe('0.90s')
  })

  it('ignores a delayed model list after switching channel drafts', async () => {
    vi.mocked(listChannels).mockResolvedValue([channel(), { ...channel(), id: 2, name: 'Second', primaryModel: 'second-model' }])
    let finish!: (result: Awaited<ReturnType<typeof fetchUpstreamModels>>) => void
    vi.mocked(fetchUpstreamModels).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels').find(item => item.gatewayId === 1))
    await flushPromises()
    wrapper.findComponent(ChannelModelManager).vm.$emit('fetch-upstream')
    await flushPromises()
    expect(wrapper.findComponent(ChannelModelManager).props('isFetching')).toBe(true)
    await wrapper.findAll('button').find(button => button.text() === '取消')!.trigger('click')
    grid.vm.$emit('edit', grid.props('channels').find(item => item.gatewayId === 2))
    await flushPromises()
    finish({ ok: true, models: ['first-draft-only'] })
    await flushPromises()
    const manager = wrapper.findComponent(ChannelModelManager)
    expect(manager.props('modelNames')).toEqual(['second-model'])
    expect(manager.props('isFetching')).toBe(false)
  })

  it.each(['url', 'protocol', 'models', 'key', 'cancel'] as const)('ignores model fetches after the edited %s changes', async change => {
    let finish!: (result: Awaited<ReturnType<typeof fetchUpstreamModels>>) => void
    vi.mocked(fetchUpstreamModels).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    if (change === 'key') {
      await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://draft.test')
      await wrapper.find('#edit-detection-api-key').setValue('draft-secret')
    }
    wrapper.findComponent(ChannelModelManager).vm.$emit('fetch-upstream')
    await flushPromises()
    if (change === 'url') await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://changed.test')
    if (change === 'protocol') await wrapper.findAll('.supplier-proto-card')[2].trigger('click')
    if (change === 'models') wrapper.findComponent(ChannelModelManager).vm.$emit('update:model-names', ['manual-model'])
    if (change === 'key') await wrapper.find('#edit-detection-api-key').setValue('another-secret')
    if (change === 'cancel') {
      await wrapper.findAll('button').find(button => button.text() === '取消')!.trigger('click')
      grid.vm.$emit('edit', grid.props('channels')[0])
    }
    await flushPromises()
    finish({ ok: true, models: ['stale-model'] })
    await flushPromises()
    const manager = wrapper.findComponent(ChannelModelManager)
    expect(manager.props('modelNames')).toEqual(change === 'models' ? ['manual-model'] : ['model'])
    expect(manager.props('isFetching')).toBe(false)
  })

  it('requires a new endpoint key and sends the explicit draft key for model listing', async () => {
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://draft.test')
    wrapper.findComponent(ChannelModelManager).vm.$emit('fetch-upstream')
    await flushPromises()
    expect(fetchUpstreamModels).not.toHaveBeenCalled()
    expect(wrapper.find('.m26-toast-text').text()).toContain('检测 Key')
    await wrapper.find('#edit-detection-api-key').setValue('draft-secret')
    wrapper.findComponent(ChannelModelManager).vm.$emit('fetch-upstream')
    await flushPromises()
    expect(fetchUpstreamModels).toHaveBeenCalledWith('https://draft.test', 'draft-secret', 'openai-compatible', 1)
    expect(wrapper.findComponent(ChannelModelManager).props('modelNames')).toEqual(['fetched-model'])
  })

  it('ignores a new-channel model response after the dialog is closed and reopened', async () => {
    let finish!: (result: Awaited<ReturnType<typeof fetchUpstreamModels>>) => void
    vi.mocked(fetchUpstreamModels).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    wrapper.findComponent(ChannelToolbar).vm.$emit('add-channel')
    await flushPromises()
    await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://new.test')
    wrapper.findComponent(ChannelModelManager).vm.$emit('fetch-upstream')
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === '取消')!.trigger('click')
    wrapper.findComponent(ChannelToolbar).vm.$emit('add-channel')
    await flushPromises()
    finish({ ok: true, models: ['stale-model'] })
    await flushPromises()
    expect(wrapper.findComponent(ChannelModelManager).props('modelNames')).not.toContain('stale-model')
    expect(wrapper.findComponent(ChannelModelManager).props('isFetching')).toBe(false)
  })

  it.each(['created', 'updated'] as const)('refreshes health after all final %s outcomes but not in-flight updates', async event => {
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    vi.useFakeTimers()
    const subscription = vi.mocked(subscribeLogs).mock.calls[0][0]
    const notify = event === 'created' ? subscription.onCreated! : subscription.onUpdated!
    const oldCalls = vi.mocked(listChannels).mock.calls.length
    notify({ id: 1, status_code: 0, is_stream: true, finish_reason: '' } as never)
    await vi.advanceTimersByTimeAsync(600)
    expect(listChannels).toHaveBeenCalledTimes(oldCalls)
    for (const [status, streaming] of [[502, true], [429, false], [200, true], [200, false]] as const) {
      vi.mocked(listChannels).mockResolvedValue([{ ...channel(), status: status >= 400 ? 'offline' : 'healthy', lastError: status >= 400 ? 'live failure' : '' }])
      notify({ id: 1, status_code: status, is_stream: streaming, finish_reason: status >= 400 ? 'error' : 'stop' } as never)
      await vi.advanceTimersByTimeAsync(500)
      await flushPromises()
      expect(wrapper.findComponent(ChannelGridView).props('channels')[0].status).toBe(status >= 400 ? 'offline' : 'healthy')
    }
    expect(listChannels).toHaveBeenCalledTimes(oldCalls + 4)
    notify({ id: 1, status_code: 502, is_stream: false } as never)
    await vi.advanceTimersByTimeAsync(500)
    expect(listChannels).toHaveBeenCalledTimes(oldCalls + 5)
  })

  it('does not show total latency as TTFT in successful log diagnostics', async () => {
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const subscription = vi.mocked(subscribeLogs).mock.calls[0][0]
    subscription.onCreated?.({ id: 1, model: 'model', status_code: 200, latency_ms: 9000, ttft_ms: 0, is_stream: false, finish_reason: 'stop' } as never)
    await wrapper.get('button[title="日志"]').trigger('click')
    await flushPromises()
    const logs = wrapper.findComponent(LogsView)
    logs.vm.$emit('open-detail', logs.props('logs')[0])
    await flushPromises()
    const detail = wrapper.findComponent(LogDetailContent)
    expect(detail.props('diagnostic').message).toContain('首字耗时 未测量')
    expect(detail.props('diagnostic').message).toContain('全链路耗时 9.00s')
  })

  it('shows live TTFT separately from probes and falls back when live data expires', async () => {
    const now = Date.now()
    vi.mocked(listChannels).mockResolvedValue([{
      ...channel(),
      modelHealth: [{ model: 'model', status: 'operational', latencyMs: 900, ttftMs: 900, error: '', checkedAt: new Date(now - 60_000).toISOString() }],
      modelPerformance: [{ model: 'model', ttftMs: 140, lastTtftMs: 300, samples: 2, updatedAt: new Date(now - 10_000).toISOString() }],
    }])
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('open-models', grid.props('channels')[0])
    await flushPromises()
    const content = wrapper.findComponent(ChannelModelLatencyContent)
    const row = content.props('rows')[0]
    expect(row.latencyTooltip).toContain('真实请求 TTFT（平滑）')
    expect(row.latencyTooltip).toContain('2 次成功样本')
    expect(row.latencyTooltip).toContain('探活 TTFT')
    expect(row.latencyDisplay).toBe('0.14s')
    expect(row.latencyClass).toBe('fast')

    const expired = { ...channel(), modelHealth: content.props('channel').modelHealth!, modelPerformance: [{ model: 'model', ttftMs: 140, lastTtftMs: 300, samples: 2, updatedAt: new Date(now - 31 * 60_000).toISOString() }] }
    vi.mocked(listChannels).mockResolvedValue([expired])
    await refresh()
    expect(content.props('rows')[0].latencyDisplay).toBe('0.90s')
    expect(content.props('rows')[0].latencyTooltip).not.toContain('真实请求')

    const subscriptions = vi.mocked(subscribeLogs).mock.calls[0][0]
    const oldCalls = vi.mocked(listChannels).mock.calls.length
    subscriptions.onUpdated?.({ status_code: 502, is_stream: true, finish_reason: 'error' } as never)
    await flushPromises()
    expect(listChannels).toHaveBeenCalledTimes(oldCalls)
    subscriptions.onUpdated?.({ status_code: 200, is_stream: true, finish_reason: 'stop' } as never)
    await new Promise(resolve => setTimeout(resolve, 550))
    await flushPromises()
    expect(listChannels).toHaveBeenCalledTimes(oldCalls + 1)
  })

  it('keeps uncertain protocol results distinct from unsupported and preserves the current selection', async () => {
    vi.mocked(detectChannelProtocol).mockResolvedValue({
      ok: true, supportsOpenai: false, supportsAnthropic: null, supportsResponses: null,
      message: 'Chat：接口不存在；Anthropic：未确认 (HTTP 401)；Responses：未确认 (HTTP 502)',
    })
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(detectChannelProtocol).toHaveBeenCalledWith('https://example.test', '', 1, 'model')
    const cards = wrapper.findAll('.supplier-proto-card')
    expect(cards[0].text()).toContain('不支持')
    expect(cards[0].classes()).toContain('active')
    for (const card of cards.slice(1)) {
      expect(card.text()).toContain('未确认')
      expect(card.text()).not.toContain('不支持')
    }
    expect(wrapper.find('.supplier-detect-result').text()).toContain('已保留当前选择')
    expect(wrapper.find('.supplier-detect-result').text()).toContain('HTTP 401')
  })

  it('selects a confirmed Responses-only endpoint', async () => {
    vi.mocked(detectChannelProtocol).mockResolvedValue({
      ok: true, detectedProtocol: 'openai-responses', supportsOpenai: false,
      supportsAnthropic: false, supportsResponses: true,
    })
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(wrapper.findAll('.supplier-proto-card')[2].classes()).toContain('active')
    expect(wrapper.find('.supplier-detect-result').text()).toContain('OpenAI Responses')
  })

  it('requires an explicit key for edited URLs and cancellation never replaces saved protocol results', async () => {
    saveChannelDetectedProtocols({ channelId: 1, url: 'https://example.test', supportsOpenai: true, supportsResponses: false })
    vi.mocked(detectChannelProtocol).mockResolvedValue({ ok: true, detectedProtocol: 'openai-responses',
      supportsOpenai: false, supportsAnthropic: false, supportsResponses: true })
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://draft.test')
    expect(wrapper.findAll('.supplier-proto-card')[0].text()).toContain('未确认')
    await wrapper.find('.supplier-detect-btn').trigger('click')
    expect(detectChannelProtocol).not.toHaveBeenCalled()
    expect(wrapper.find('.supplier-detect-result').text()).toContain('检测 Key')
    await wrapper.find('#edit-detection-api-key').setValue('draft-secret')
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(detectChannelProtocol).toHaveBeenCalledWith('https://draft.test', 'draft-secret', 1, 'model')
    expect(wrapper.findAll('.supplier-proto-card')[2].text()).toContain('支持')
    await wrapper.findAll('button').find(button => button.text() === '取消')!.trigger('click')
    expect(getChannelDetectedProtocols(1, 'https://example.test')?.supportsOpenai).toBe(true)
    expect(getChannelDetectedProtocols(undefined, 'https://draft.test')).toBeNull()
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    expect(wrapper.findAll('.supplier-proto-card')[0].text()).toContain('支持')
    expect(wrapper.findAll('.supplier-proto-card')[2].text()).toContain('不支持')
  })

  it('does not persist detection results from an unsaved new channel', async () => {
    vi.mocked(detectChannelProtocol).mockResolvedValue({ ok: true, supportsOpenai: true, supportsResponses: false })
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    wrapper.findComponent(ChannelToolbar).vm.$emit('add-channel')
    await flushPromises()
    await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://new.test')
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(wrapper.findAll('.supplier-proto-card')[0].text()).toContain('支持')
    expect(getChannelDetectedProtocols(undefined, 'https://new.test')).toBeNull()
  })

  it.each(['pending', 'completed'] as const)('invalidates %s new-channel protocol detection when the effective key changes', async phase => {
    let finish!: (result: Awaited<ReturnType<typeof detectChannelProtocol>>) => void
    const oldResult = { ok: true, detectedProtocol: 'openai-responses' as const,
      supportsOpenai: false, supportsAnthropic: false, supportsResponses: true }
    if (phase === 'pending') vi.mocked(detectChannelProtocol).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    else vi.mocked(detectChannelProtocol).mockResolvedValueOnce(oldResult)
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    wrapper.findComponent(ChannelToolbar).vm.$emit('add-channel')
    await flushPromises()
    await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://new.test')
    const keys = wrapper.findComponent(ChannelKeyInputs)
    keys.vm.$emit('update:modelValue', [{ name: 'Key 1', secret: '' }, { name: 'Key 2', secret: 'old-secret' }])
    await flushPromises()
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(detectChannelProtocol).toHaveBeenLastCalledWith('https://new.test', 'old-secret', undefined, undefined)
    keys.vm.$emit('update:modelValue', [{ name: 'Key 1', secret: 'new-secret' }, { name: 'Key 2', secret: 'old-secret' }])
    await flushPromises()
    expect(wrapper.find('.supplier-detect-btn').attributes('disabled')).toBeUndefined()
    expect(wrapper.find('.supplier-detect-result').exists()).toBe(false)
    for (const card of wrapper.findAll('.supplier-proto-card')) expect(card.text()).toContain('未确认')

    vi.mocked(detectChannelProtocol).mockResolvedValueOnce({ ok: true, detectedProtocol: 'openai-compatible',
      supportsOpenai: true, supportsAnthropic: false, supportsResponses: false })
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(detectChannelProtocol).toHaveBeenLastCalledWith('https://new.test', 'new-secret', undefined, undefined)
    if (phase === 'pending') {
      finish(oldResult)
      await flushPromises()
    }
    const cards = wrapper.findAll('.supplier-proto-card')
    expect(cards[0].classes()).toContain('active')
    expect(cards[0].text()).toContain('支持')
    expect(cards[2].classes()).not.toContain('active')
    expect(cards[2].text()).toContain('不支持')
    expect(getChannelDetectedProtocols(undefined, 'https://new.test')).toBeNull()
  })

  it.each(['url', 'cancel'] as const)('ignores detection responses arriving after %s changes', async (change) => {
    let resolveDetection!: (value: Awaited<ReturnType<typeof detectChannelProtocol>>) => void
    vi.mocked(detectChannelProtocol).mockImplementationOnce(() => new Promise(resolve => { resolveDetection = resolve }))
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    await wrapper.find('.supplier-detect-btn').trigger('click')
    if (change === 'url') await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://another.test')
    else {
      await wrapper.findAll('button').find(button => button.text() === '取消')!.trigger('click')
      grid.vm.$emit('edit', grid.props('channels')[0])
      await flushPromises()
    }
    resolveDetection({ ok: true, detectedProtocol: 'openai-responses', supportsResponses: true, supportsOpenai: false })
    await flushPromises()
    expect(wrapper.findAll('.supplier-proto-card')[2].classes()).not.toContain('active')
    expect(wrapper.find('.supplier-detect-result').exists()).toBe(false)
    expect(getChannelDetectedProtocols(1, 'https://example.test')).toBeNull()
  })

  it('persists draft protocol results only after the edited channel is saved', async () => {
    vi.mocked(detectChannelProtocol).mockResolvedValue({ ok: true, supportsOpenai: false, supportsResponses: true, detectedProtocol: 'openai-responses' })
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('edit', grid.props('channels')[0])
    await flushPromises()
    await wrapper.find('input[placeholder="https://api.openai.com"]').setValue('https://draft.test')
    await wrapper.find('#edit-detection-api-key').setValue('draft-secret')
    await wrapper.find('.supplier-detect-btn').trigger('click')
    await flushPromises()
    expect(getChannelDetectedProtocols(1, 'https://draft.test')).toBeNull()
    vi.mocked(updateChannel).mockResolvedValue({ ...channel(), url: 'https://draft.test', provider: 'openai-responses' })
    await wrapper.findAll('button').find(button => button.text() === '保存修改')!.trigger('click')
    await flushPromises()
    expect(getChannelDetectedProtocols(1, 'https://draft.test')?.supportsResponses).toBe(true)
    expect(updateChannel).toHaveBeenCalledWith(expect.objectContaining({ url: 'https://draft.test', provider: 'openai-responses', apiKey: '' }))
  })

  it('rejects local-only mutations after a failed refresh and saves through the API after reconnecting', async () => {
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    expect(grid.props('channels')).toHaveLength(1)
    vi.mocked(listChannels).mockResolvedValue(null)
    await refresh()
    grid.vm.$emit('toggle', grid.props('channels')[0])
    await flushPromises()
    expect(grid.props('channels')[0].enabled).toBe(true)
    expect(wrapper.find('.m26-toast-text').text()).toContain('未保存')
    expect(updateChannel).not.toHaveBeenCalled()
    expect(writeValue).not.toHaveBeenCalled()

    grid.vm.$emit('duplicate', grid.props('channels')[0])
    await flushPromises()
    expect(grid.props('channels')).toHaveLength(1)
    expect(duplicateChannel).not.toHaveBeenCalled()
    expect(wrapper.find('.m26-toast-text').text()).toContain('无法复制')

    grid.vm.$emit('remove', grid.props('channels')[0])
    await flushPromises()
    const confirm = wrapper.findAll('button').find(button => button.text() === '确认删除')!
    expect(confirm).toBeDefined()
    await confirm.trigger('click')
    await flushPromises()
    expect(deleteChannel).not.toHaveBeenCalled()
    expect(grid.props('channels')).toHaveLength(1)
    expect(wrapper.find('.m26-toast-text').text()).toContain('未删除')

    vi.mocked(listChannels).mockResolvedValue([channel()])
    await refresh()
    vi.mocked(updateChannel).mockResolvedValue({ ...channel(), enabled: false })
    grid.vm.$emit('toggle', grid.props('channels')[0])
    await flushPromises()
    expect(updateChannel).toHaveBeenCalledWith(expect.objectContaining({ id: 1, enabled: false }))
    expect(grid.props('channels')[0].enabled).toBe(false)
    expect(writeValue).not.toHaveBeenCalled()
  })

  it('rolls back the displayed switch when an API save fails', async () => {
    const wrapper = shallowMount(GatewayView)
    await flushPromises()
    const grid = wrapper.findComponent(ChannelGridView)
    grid.vm.$emit('toggle', grid.props('channels')[0])
    await flushPromises()
    expect(updateChannel).toHaveBeenCalledOnce()
    expect(grid.props('channels')[0].enabled).toBe(true)
    expect(wrapper.find('.m26-toast-text').text()).toContain('保存失败')
    expect(writeValue).not.toHaveBeenCalled()
  })
})

describe('GatewayView settings persistence', () => {
  for (const key of ['log_retention_days', 'log_max_count', 'hide_dock_icon'] as const) {
    it.each([[false, true], [true, false], [false, false], [true, true]])(
      `${key} serializes rapid edits and rolls back to the last confirmed value (%s, %s)`, async (firstOk, secondOk) => {
        const wrapper = shallowMount(GatewayView, { global: { stubs: { StorageSettings: false } } })
        await flushPromises()
        window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'settings' }))
        await flushPromises()
        const storage = wrapper.findComponent(StorageSettings)
        const general = wrapper.findComponent(DesktopGeneralSettings)
        const current = () => key === 'hide_dock_icon' ? general.props('hideDockIcon')
          : storage.props(key === 'log_retention_days' ? 'logRetentionDays' : 'logMaxCount')
        const initial = current()
        const first = key === 'hide_dock_icon' ? true : key === 'log_retention_days' ? 30 : 1000
        const second = key === 'hide_dock_icon' ? false : key === 'log_retention_days' ? 14 : 3000
        const edit = async (value: typeof first | typeof second) => {
          if (key === 'hide_dock_icon') general.vm.$emit('update:hide-dock-icon', value)
          else await storage.findAll('select')[key === 'log_retention_days' ? 0 : 1]!.setValue(String(value))
          await flushPromises()
        }
        let finishFirst!: (ok: boolean) => void
        let finishSecond!: (ok: boolean) => void
        vi.mocked(updateSettings)
          .mockImplementationOnce(() => new Promise<boolean>(resolve => { finishFirst = resolve }))
          .mockImplementationOnce(() => new Promise<boolean>(resolve => { finishSecond = resolve }))
        await edit(first)
        await edit(second)
        expect(current()).toBe(second)
        expect(updateSettings).toHaveBeenCalledTimes(1)
        finishFirst(firstOk)
        await flushPromises()
        expect(updateSettings).toHaveBeenCalledTimes(2)
        expect(updateSettings).toHaveBeenNthCalledWith(1, { [key]: first })
        expect(updateSettings).toHaveBeenNthCalledWith(2, { [key]: second })
        expect(current()).toBe(second)
        finishSecond(secondOk)
        await flushPromises()
        const expected = secondOk ? second : firstOk ? first : initial
        expect(current()).toBe(expected)
        if (key === 'log_retention_days' || key === 'log_max_count') {
          expect((storage.findAll('select')[key === 'log_retention_days' ? 0 : 1]!.element as HTMLSelectElement).value).toBe(String(expected))
        }
        if (!secondOk) expect(wrapper.find('.m26-toast-text').text()).toContain('未保存')
      },
    )
  }

  it('ignores a system refresh started before a newer setting edit', async () => {
    const wrapper = shallowMount(GatewayView, { global: { stubs: { StorageSettings: false } } })
    await flushPromises()
    window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'settings' }))
    await flushPromises()
    const storage = wrapper.findComponent(StorageSettings)
    let finishRefresh!: (info: SystemInfo) => void
    vi.mocked(getSystemInfo).mockImplementationOnce(() => new Promise<SystemInfo>(resolve => { finishRefresh = resolve }))
    vi.mocked(updateSettings).mockResolvedValue(true)
    await storage.findAll('select')[0]!.setValue('30')
    await flushPromises()
    await storage.findAll('select')[0]!.setValue('14')
    await flushPromises()
    finishRefresh({ data_dir: '', db_path: '', db_size_bytes: 0, logs_count: 0,
      log_retention_days: 30, log_max_count: 5000, routing_strategy: 'latency' })
    await flushPromises()
    expect(storage.props('logRetentionDays')).toBe(14)
    expect(updateSettings).toHaveBeenNthCalledWith(2, { log_retention_days: 14 })
    expect((storage.findAll('select')[0]!.element as HTMLSelectElement).value).toBe('14')
  })

  it('rolls back a failed field even when another setting succeeds', async () => {
    const wrapper = shallowMount(GatewayView, { global: { stubs: { StorageSettings: false } } })
    await flushPromises()
    window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'settings' }))
    await flushPromises()
    const storage = wrapper.findComponent(StorageSettings)
    let failRetention!: (ok: boolean) => void
    vi.mocked(updateSettings)
      .mockImplementationOnce(() => new Promise<boolean>(resolve => { failRetention = resolve }))
      .mockResolvedValueOnce(true)
    await storage.findAll('select')[0]!.setValue('30')
    await storage.findAll('select')[1]!.setValue('1000')
    await flushPromises()
    failRetention(false)
    await flushPromises()
    expect(storage.props('logRetentionDays')).toBe(7)
    expect(storage.props('logMaxCount')).toBe(1000)
  })

  it.each(['disconnected', 'rejected', 'saved'])('keeps settings consistent when %s', async (mode) => {
    const wrapper = shallowMount(GatewayView, { global: { stubs: { StorageSettings: false } } })
    await flushPromises()
    window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'settings' }))
    await flushPromises()
    const storage = wrapper.findComponent(StorageSettings)
    const general = wrapper.findComponent(DesktopGeneralSettings)
    if (mode === 'disconnected') {
      vi.mocked(listChannels).mockResolvedValue(null)
      await refresh()
    }
    vi.mocked(updateSettings).mockResolvedValue(mode === 'saved')
    const initialRetention = storage.props('logRetentionDays')
    const initialMaxCount = storage.props('logMaxCount')
    const selects = storage.findAll('select')
    await selects[0]!.setValue('30')
    await flushPromises()
    expect(storage.props('logRetentionDays')).toBe(mode === 'saved' ? 30 : initialRetention)
    expect((selects[0]!.element as HTMLSelectElement).value).toBe(String(mode === 'saved' ? 30 : initialRetention))
    if (mode !== 'saved') expect(wrapper.find('.m26-toast-text').text()).toContain('未保存')
    await selects[1]!.setValue('1000')
    await flushPromises()
    expect(storage.props('logMaxCount')).toBe(mode === 'saved' ? 1000 : initialMaxCount)
    expect((selects[1]!.element as HTMLSelectElement).value).toBe(String(mode === 'saved' ? 1000 : initialMaxCount))
    if (mode !== 'saved') expect(wrapper.find('.m26-toast-text').text()).toContain('未保存')
    general.vm.$emit('update:hide-dock-icon', true)
    await flushPromises()
    expect(general.props('hideDockIcon')).toBe(mode === 'saved')

    expect(updateSettings).toHaveBeenCalledWith({ log_retention_days: 30 })
    expect(updateSettings).toHaveBeenCalledWith({ log_max_count: 1000 })
    if (mode === 'rejected') expect(wrapper.find('.m26-toast-text').text()).toContain('未保存')
    expect(writeValue).not.toHaveBeenCalled()
  })
})
