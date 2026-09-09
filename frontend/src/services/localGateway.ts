const DEFAULT_GATEWAY_URL = ''

export type LocalGatewayHealth = {
  ok: boolean
  latency: number | null
  url: string
}

export async function checkLocalGateway(baseUrl = DEFAULT_GATEWAY_URL): Promise<LocalGatewayHealth> {
  const url = baseUrl ? `${baseUrl.replace(/\/$/, '')}/health` : '/health'
  const startedAt = performance.now()
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), 2500)

  try {
    const response = await fetch(url, { signal: controller.signal, headers: { Accept: 'application/json' } })
    return { ok: response.ok, latency: Math.round(performance.now() - startedAt), url: baseUrl || window.location.origin }
  } catch {
    return { ok: false, latency: null, url: baseUrl || window.location.origin }
  } finally {
    window.clearTimeout(timeout)
  }
}
