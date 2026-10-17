export type DetectedProtocolInfo = {
  supportsOpenai: boolean | null
  supportsAnthropic: boolean | null
  supportsResponses: boolean | null
  detectedProtocol?: string
  models?: string[]
  updatedAt?: number
  endpoint?: string
}

const STORAGE_KEY = 'heigate_channel_detected_protocols_v1'

let cacheMap: Record<string, DetectedProtocolInfo> | null = null

function loadStorage(): Record<string, DetectedProtocolInfo> {
  if (cacheMap !== null) return cacheMap
  cacheMap = {}
  if (typeof localStorage === 'undefined') return cacheMap
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      cacheMap = JSON.parse(raw) as Record<string, DetectedProtocolInfo>
    }
  } catch {
    cacheMap = {}
  }
  return cacheMap
}

function persistStorage(): void {
  if (!cacheMap || typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cacheMap))
  } catch {
    // Ignore storage quota or serialization errors
  }
}

export function normalizeUrlKey(url?: string): string {
  if (!url) return ''
  let u = url.trim()
  if (!/^https?:\/\//i.test(u)) {
    u = 'https://' + u
  }
  try {
    return new URL(u).toString().replace(/\/+$/, '')
  } catch {
    return u.replace(/\/+$/, '')
  }
}

export function getChannelDetectedProtocols(channelId?: number, url?: string): DetectedProtocolInfo | null {
  const store = loadStorage()
  const normUrl = normalizeUrlKey(url)
  if (channelId != null && channelId > 0) {
    const byId = store[`id:${channelId}`]
    if (byId && (!normUrl || byId.endpoint === normUrl)) return byId
  }
  if (normUrl) {
    const byNorm = store[`url:${normUrl}`]
    if (byNorm) return byNorm
  }
  if (url && url.trim()) {
    const byRaw = store[`url:${url.trim()}`]
    if (byRaw) return byRaw
  }
  return null
}

export function saveChannelDetectedProtocols(params: {
  channelId?: number
  url?: string
  supportsOpenai?: boolean | null
  supportsAnthropic?: boolean | null
  supportsResponses?: boolean | null
  detectedProtocol?: string
  models?: string[]
}): DetectedProtocolInfo {
  const store = loadStorage()
  const existing = getChannelDetectedProtocols(params.channelId, params.url)

  const info: DetectedProtocolInfo = {
    endpoint: normalizeUrlKey(params.url) || existing?.endpoint,
    supportsOpenai: params.supportsOpenai !== undefined ? params.supportsOpenai : (existing?.supportsOpenai ?? null),
    supportsAnthropic: params.supportsAnthropic !== undefined ? params.supportsAnthropic : (existing?.supportsAnthropic ?? null),
    supportsResponses: params.supportsResponses !== undefined ? params.supportsResponses : (existing?.supportsResponses ?? null),
    detectedProtocol: params.detectedProtocol || existing?.detectedProtocol,
    models: params.models ?? existing?.models,
    updatedAt: Date.now(),
  }

  if (params.channelId != null && params.channelId > 0) {
    store[`id:${params.channelId}`] = info
  }
  const normUrl = normalizeUrlKey(params.url)
  if (normUrl) {
    store[`url:${normUrl}`] = info
  }
  if (params.url && params.url.trim()) {
    store[`url:${params.url.trim()}`] = info
  }

  persistStorage()
  return info
}

export function linkChannelIdProtocols(channelId: number, url: string): void {
  if (!channelId || channelId <= 0 || !url) return
  const store = loadStorage()
  if (store[`id:${channelId}`]?.endpoint === normalizeUrlKey(url)) return
  const existing = getChannelDetectedProtocols(undefined, url)
  if (existing) {
    store[`id:${channelId}`] = { ...existing, endpoint: normalizeUrlKey(url) }
  } else {
    delete store[`id:${channelId}`]
  }
  persistStorage()
}

export function clearChannelDetectedProtocols(channelId?: number, url?: string): void {
  const store = loadStorage()
  if (channelId != null && channelId > 0) {
    delete store[`id:${channelId}`]
  }
  const normUrl = normalizeUrlKey(url)
  if (normUrl) {
    delete store[`url:${normUrl}`]
  }
  if (url && url.trim()) {
    delete store[`url:${url.trim()}`]
  }
  persistStorage()
}

/** Reset in-memory cache (mainly for unit tests) */
export function _resetDetectedProtocolsCacheForTest(): void {
  cacheMap = null
}
