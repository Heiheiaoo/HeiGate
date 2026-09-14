import { afterEach, describe, expect, it, vi } from 'vitest'
import { createChannel, listChannels } from '@/services/gatewayApi'

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
})
