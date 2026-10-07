import { describe, it, expect, beforeEach } from 'vitest'
import {
  getChannelDetectedProtocols,
  saveChannelDetectedProtocols,
  linkChannelIdProtocols,
  normalizeUrlKey,
  _resetDetectedProtocolsCacheForTest,
} from './channelProtocols'

const storageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value.toString()
    },
    removeItem: (key: string) => {
      delete store[key]
    },
    clear: () => {
      store = {}
    },
  }
})()

describe('channelProtocols', () => {
  beforeEach(() => {
    Object.defineProperty(globalThis, 'localStorage', {
      value: storageMock,
      writable: true,
      configurable: true,
    })
    localStorage.clear()
    _resetDetectedProtocolsCacheForTest()
  })

  it('normalizes URL key correctly', () => {
    expect(normalizeUrlKey('https://api.openai.com/v1/')).toBe('https://api.openai.com/v1')
    expect(normalizeUrlKey('http://API.ANTHROPIC.COM/')).toBe('http://api.anthropic.com')
    expect(normalizeUrlKey('api.deepseek.com')).toBe('https://api.deepseek.com')
  })

  it('saves and retrieves detected protocols by URL and channelId', () => {
    saveChannelDetectedProtocols({
      channelId: 42,
      url: 'https://api.example.com/v1/',
      supportsOpenai: true,
      supportsAnthropic: true,
      supportsResponses: false,
      detectedProtocol: 'openai-compatible',
    })

    // Retrieve by ID
    const byId = getChannelDetectedProtocols(42)
    expect(byId).not.toBeNull()
    expect(byId?.supportsOpenai).toBe(true)
    expect(byId?.supportsAnthropic).toBe(true)
    expect(byId?.supportsResponses).toBe(false)
    expect(byId?.detectedProtocol).toBe('openai-compatible')

    // Retrieve by normalized URL
    const byUrl = getChannelDetectedProtocols(undefined, 'https://api.example.com/v1')
    expect(byUrl).toEqual(byId)

    // Retrieve by raw URL
    const byRawUrl = getChannelDetectedProtocols(undefined, 'https://api.example.com/v1/')
    expect(byRawUrl).toEqual(byId)
  })

  it('survives cache reload from localStorage', () => {
    saveChannelDetectedProtocols({
      channelId: 99,
      url: 'https://api.test.com',
      supportsOpenai: true,
      supportsAnthropic: false,
      supportsResponses: null,
    })

    // Reset memory mirror to simulate page reload / app restart
    _resetDetectedProtocolsCacheForTest()

    const restored = getChannelDetectedProtocols(99)
    expect(restored).not.toBeNull()
    expect(restored?.supportsOpenai).toBe(true)
    expect(restored?.supportsAnthropic).toBe(false)
    expect(restored?.supportsResponses).toBeNull()
  })

  it('links channelId to previously URL-probed protocols', () => {
    saveChannelDetectedProtocols({
      url: 'https://api.newchannel.com/v1',
      supportsOpenai: true,
      supportsAnthropic: true,
      supportsResponses: false,
    })

    linkChannelIdProtocols(123, 'https://api.newchannel.com/v1/')

    const byId = getChannelDetectedProtocols(123)
    expect(byId).not.toBeNull()
    expect(byId?.supportsOpenai).toBe(true)
    expect(byId?.supportsAnthropic).toBe(true)
  })
})
