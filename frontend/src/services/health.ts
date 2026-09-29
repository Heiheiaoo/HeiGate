const DEFAULT_GATEWAY_URL = ''

export type GatewayHealth = {
  ok: boolean
  latency: number | null
  url: string
  version: string | null
}

export async function checkGatewayHealth(baseUrl = DEFAULT_GATEWAY_URL): Promise<GatewayHealth> {
  const url = baseUrl ? `${baseUrl.replace(/\/$/, '')}/health` : '/health'
  const startedAt = performance.now()
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), 2500)

  try {
    const response = await fetch(url, { signal: controller.signal, headers: { Accept: 'application/json' } })
    const payload: unknown = response.ok ? await response.json().catch(() => null) : null
    const version = payload && typeof payload === 'object' && 'version' in payload && typeof payload.version === 'string'
      ? payload.version
      : null
    return {
      ok: response.ok,
      latency: Math.round(performance.now() - startedAt),
      url: baseUrl || window.location.origin,
      version,
    }
  } catch {
    return { ok: false, latency: null, url: baseUrl || window.location.origin, version: null }
  } finally {
    window.clearTimeout(timeout)
  }
}
