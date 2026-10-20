import { afterEach, describe, expect, it, vi } from 'vitest'
import { createChannel, listChannels, probeChannelDetails, injectionCheck, updateClientConfig, detectChannelProtocol } from '@/services/gatewayApi'

type FetchArgs = [input: string, init?: RequestInit]

function mockFetch(response: unknown, ok = true) {
  const spy = vi.fn(async () => ({
    ok,
    status: ok ? 200 : 500,
    json: async () => response,
  }))
  vi.stubGlobal('fetch', spy)
  return spy
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('gatewayApi', () => {
  it('keeps live model performance separate from probe health', async () => {
    mockFetch({ data: [{
      id: 1, model_health: [{ model: 'extra', ttft_ms: 900 }],
      model_performance: [{ model: 'extra', ttft_ms: 140, last_ttft_ms: 300, samples: 2, updated_at: '2026-10-08T00:00:00Z' }],
    }] })
    const channels = await listChannels()
    expect(channels?.[0].modelHealth[0].ttftMs).toBe(900)
    expect(channels?.[0].modelPerformance?.[0]).toEqual({ model: 'extra', ttftMs: 140, lastTtftMs: 300, samples: 2, updatedAt: '2026-10-08T00:00:00Z' })
  })

  it('passes the selected model and preserves uncertain protocol detection', async () => {
    const spy = mockFetch({ data: {
      detected_protocol: null, supports_openai: false, supports_anthropic: null,
      supports_responses: null, models: [], message: '未确认',
    } })
    const result = await detectChannelProtocol('https://supplier.test', '', 7, 'qwen3.8-27b')
    expect(result).toMatchObject({ ok: true, detectedProtocol: undefined, supportsOpenai: false, supportsAnthropic: null, supportsResponses: null })
    expect(JSON.parse((spy.mock.calls[0] as unknown as FetchArgs)[1]!.body as string)).toEqual({
      endpoint: 'https://supplier.test', channel_id: 7, model: 'qwen3.8-27b',
    })
  })

  it('does not discard an explicit draft detection credential when channelId is provided', async () => {
    const spy = mockFetch({ data: { detected_protocol: null, supports_openai: null, supports_anthropic: null, models: [], message: '' } })
    await detectChannelProtocol('https://draft.test', 'draft-secret', 7, 'model')
    expect(JSON.parse((spy.mock.calls[0] as unknown as FetchArgs)[1]!.body as string)).toMatchObject({
      endpoint: 'https://draft.test', api_key: 'draft-secret', channel_id: 7,
    })
  })

  it('reads channels from the local management API', async () => {
    const spy = mockFetch({
      data: [
        {
          id: 7,
          name: 'local',
          provider: 'openai',
          endpoint: 'https://api.example.com',
          primary_model: 'gpt-x',
          enabled: true,
          last_status: 'operational',
        },
      ],
    })

    const channels = await listChannels()
    const [url, init] = spy.mock.calls[0] as unknown as FetchArgs

    expect(url).toBe('/api/channels?include_secrets=1')
    expect((init?.headers as Record<string, string>)['X-HeiGate-Request']).toBeUndefined()
    expect(channels).toHaveLength(1)
    expect(channels?.[0].name).toBe('local')
  })

  it('sends the anti-CSRF header on writes', async () => {
    const spy = mockFetch({ data: { id: 1, name: 'new', provider: 'openai' } })

    await createChannel({ name: 'new', provider: 'openai', apiKey: 'sk-x' } as never)
    const [url, init] = spy.mock.calls[0] as unknown as FetchArgs
    const headers = init?.headers as Record<string, string>

    expect(url).toBe('/api/channels')
    expect(init?.method).toBe('POST')
    expect(headers['X-HeiGate-Request']).toBe('1')
    expect(headers['Content-Type']).toBe('application/json')
  })

  it('returns null instead of throwing when the gateway is unreachable', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => {
      throw new Error('connection refused')
    }))
    await expect(listChannels()).resolves.toBeNull()
  })

  it('addresses single-model and explicit deep probes separately', async () => {
    const spy = mockFetch({ data: [] })

    await probeChannelDetails(7, 'model x')
    await probeChannelDetails(7, undefined, { all: true, modelCount: 3 })

    const [singleUrl] = spy.mock.calls[0] as unknown as FetchArgs
    const [allUrl] = spy.mock.calls[1] as unknown as FetchArgs
    expect(singleUrl).toBe('/api/channels/7/probe?model=model%20x')
    expect(allUrl).toBe('/api/channels/7/probe?all=1')
  })

  it('keeps probe start timestamps for rejecting stale diagnostic results', async () => {
    mockFetch({ data: [{ model: 'model', status: 'operational', checked_at: '2026-10-08T10:00:00Z' }] })
    const results = await probeChannelDetails(7, 'model')
    expect(results).toEqual([expect.objectContaining({ checkedAt: '2026-10-08T10:00:00Z' })])
  })

  it.each([true, false, undefined])('preserves the probe acceptance flag %s without inventing it for old backends', async accepted => {
    mockFetch({ data: [{ model: 'model', status: 'operational', accepted }] })
    const results = await probeChannelDetails(7, 'model')
    expect(Array.isArray(results)).toBe(true)
    const result = Array.isArray(results) ? results[0] : null
    if (accepted === undefined) expect(result).not.toHaveProperty('accepted')
    else expect(result).toHaveProperty('accepted', accepted)
  })

  it.each([true, false])('preserves injection diagnostic acceptance %s', async accepted => {
    mockFetch({ data: { results: [{ model: 'model', state: 'clean', prompt_tokens: 20, accepted }] } })
    expect(await injectionCheck(7, { model: 'model' })).toEqual([expect.objectContaining({ accepted })])
  })
})

describe('client operations', () => {
  it('reads the nested action message from a successful save', async () => {
    mockFetch({ data: { status: 'ok', message: '已重启 Hermes' } })
    await expect(updateClientConfig('hermes', {})).resolves.toEqual({ ok: true, message: '已重启 Hermes' })
  })

  it('surfaces action and validation failures instead of assuming save success', async () => {
    mockFetch({ error: '网关设置已保存，但本地操作未完成：重启失败' }, false)
    await expect(updateClientConfig('hermes', {})).resolves.toMatchObject({ ok: false, message: expect.stringContaining('重启失败') })
    mockFetch({ data: { status: 'error', message: '失败' } })
    await expect(updateClientConfig('hermes', {})).resolves.toMatchObject({ ok: false })
  })


})
