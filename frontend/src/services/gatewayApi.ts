export type ApiChannel = {
  keyCount?: number
  keyAvailableCount?: number
  keyCoolingCount?: number
  id: number
  name: string
  provider: 'anthropic' | 'openai-compatible' | 'openai-responses'
  apiMode: 'chat_completions' | 'responses'
  url: string
  apiKey?: string
  primaryModel: string
  extraModels: string[]
  enabled: boolean
  status: 'healthy' | 'degraded' | 'offline' | 'unknown'
  latency: number | null
  ttft: number | null
  pingLatency: number | null
  lastError: string
  lastCheck: string
  failureCount: number
  circuitState: 'closed' | 'open' | 'half_open'
  cooldownUntil: string | null
  injectionState: 'unknown' | 'clean' | 'suspected' | 'detected' | 'error'
  injectionTokens: number | null
  injectionContent: string
  injectionCheckedAt: string | null
  modelHealth: Array<{
    model: string
    status: string
    latencyMs: number | null
    ttftMs: number | null
    error: string
    checkedAt: string | null
  }>
  modelPerformance?: Array<{
    model: string
    ttftMs: number
    lastTtftMs: number
    samples: number
    updatedAt: string
  }>
}

type ApiRecord = Record<string, unknown>

function asRecord(value: unknown): ApiRecord {
  return typeof value === 'object' && value !== null ? value as ApiRecord : {}
}

function numberOrNull(value: unknown): number | null {
  return value == null ? null : Number(value)
}

const endpoint = '/api'

function protocolFromRecord(record: ApiRecord): ApiChannel['provider'] {
  if (record.provider === 'anthropic') return 'anthropic'
  if (record.api_mode === 'responses') return 'openai-responses'
  return 'openai-compatible'
}

function normalizeChannel(record: ApiRecord): ApiChannel {
  const status = record.last_status === 'operational' ? 'healthy' : record.last_status === 'degraded' ? 'degraded' : record.last_status === 'error' || record.last_status === 'failed' ? 'offline' : 'unknown'
  return {
    keyCount: Number(record.key_count ?? 1),
    keyAvailableCount: Number(record.key_available_count ?? 1),
    keyCoolingCount: Number(record.key_cooling_count ?? 0),
    id: Number(record.id),
    name: String(record.name ?? ''),
    provider: protocolFromRecord(record),
    apiMode: record.api_mode === 'responses' ? 'responses' : 'chat_completions',
    url: String(record.endpoint ?? ''),
    apiKey: typeof record.api_key === 'string' ? record.api_key : undefined,
    primaryModel: String(record.primary_model ?? ''),
    extraModels: Array.isArray(record.extra_models) ? record.extra_models.map(String) : [],
    enabled: Boolean(record.enabled),
    status,
    latency: record.last_latency_ms == null ? null : Number(record.last_latency_ms),
    ttft: record.last_ttft_ms == null ? null : Number(record.last_ttft_ms),
    pingLatency: record.last_ping_latency_ms == null ? null : Number(record.last_ping_latency_ms),
    lastError: String(record.last_error ?? ''),
    lastCheck: record.last_checked_at ? String(record.last_checked_at) : '尚未探活',
    failureCount: Number(record.failure_count ?? 0),
    circuitState: record.circuit_state === 'open' || record.circuit_state === 'half_open' ? record.circuit_state : 'closed',
    cooldownUntil: record.cooldown_until ? String(record.cooldown_until) : null,
    injectionState: ['clean', 'suspected', 'detected', 'error'].includes(String(record.injection_state)) ? record.injection_state as ApiChannel['injectionState'] : 'unknown',
    injectionTokens: record.injection_tokens == null ? null : Number(record.injection_tokens),
    injectionContent: String(record.injection_content ?? ''),
    injectionCheckedAt: record.injection_checked_at ? String(record.injection_checked_at) : null,
    modelHealth: Array.isArray(record.model_health) ? record.model_health.map((rawItem: unknown) => {
      const item = asRecord(rawItem)
      return {
        model: String(item.model ?? ''),
        status: String(item.status ?? 'unknown'),
        latencyMs: item.latency_ms == null ? null : Number(item.latency_ms),
        ttftMs: item.ttft_ms == null ? null : Number(item.ttft_ms),
        error: String(item.error ?? ''),
        checkedAt: item.checked_at ? String(item.checked_at) : null,
      }
    }) : [],
    modelPerformance: Array.isArray(record.model_performance) ? record.model_performance.map((rawItem: unknown) => {
      const item = asRecord(rawItem)
      return { model: String(item.model ?? ''), ttftMs: Number(item.ttft_ms), lastTtftMs: Number(item.last_ttft_ms), samples: Number(item.samples), updatedAt: String(item.updated_at ?? '') }
    }) : [],
  }
}

async function request(path: string, init?: RequestInit, timeoutMs = 30_000): Promise<Response> {
  const method = (init?.method ?? 'GET').toUpperCase()
  const headers: Record<string, string> = {
    Accept: 'application/json',
    // The gateway requires this header on every state-changing call: a browser
    // cannot add it cross-origin without a preflight, which blocks CSRF.
    ...(method === 'GET' || method === 'HEAD' ? {} : { 'X-HeiGate-Request': '1' }),
    ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
    ...((init?.headers as Record<string, string>) ?? {}),
  }
  // Without a deadline a wedged gateway hangs the caller forever (e.g. the
  // probing flag that is cleared only after the response arrives).
  return fetch(`${endpoint}${path}`, { ...init, headers, credentials: 'same-origin', signal: AbortSignal.timeout(timeoutMs) })
}

export async function listChannels(includeSecrets = true): Promise<ApiChannel[] | null> {
  try {
    const response = await request(`/channels${includeSecrets ? '?include_secrets=1' : ''}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord[] }
    return Array.isArray(payload.data) ? payload.data.map(normalizeChannel) : []
  } catch {
    return null
  }
}

export async function getChannel(id: number, includeSecrets = true): Promise<ApiChannel | null> {
  try {
    const response = await request(`/channels/${id}${includeSecrets ? '?include_secrets=1' : ''}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord }
    return payload.data ? normalizeChannel(payload.data) : null
  } catch {
    return null
  }
}

export async function createChannel(channel: Partial<ApiChannel> & { apiKey: string }): Promise<ApiChannel | null> {
  try {
    const response = await request('/channels', { method: 'POST', body: JSON.stringify(toPayload(channel)) })
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord }
    return payload.data ? normalizeChannel(payload.data) : null
  } catch {
    return null
  }
}

export async function updateChannel(channel: ApiChannel): Promise<ApiChannel | null> {
  try {
    const payload = toPayload(channel)
    if (!channel.apiKey || channel.apiKey.includes('***')) delete payload.api_key
    const response = await request(`/channels/${channel.id}`, { method: 'PUT', body: JSON.stringify(payload) })
    if (!response.ok) return null
    const body = await response.json() as { data?: ApiRecord }
    return body.data ? normalizeChannel(body.data) : null
  } catch {
    return null
  }
}

export async function deleteChannel(id: number): Promise<boolean> {
  try {
    const response = await request(`/channels/${id}`, { method: 'DELETE' })
    return response.ok
  } catch {
    return false
  }
}

export type ProbeResult = {
  model: string
  accepted?: boolean
  checkedAt?: string
  status: string
  latencyMs?: number | null
  ttftMs?: number | null
  pingLatencyMs?: number | null
  message?: string
}

// Must exceed the server's worst-case single-model probe budget (probe.RunBudgetFor(1)
// = 70s: one 60s attempt + 10s slack).
const PROBE_TIMEOUT_MS = 75_000

function probeTimeoutMs(modelCount = 1): number {
  // Deep scans are explicit and sequential on the server. Keep a little more
  // client-side slack than the Go budget: N×60s attempts + 20s.
  return Math.max(PROBE_TIMEOUT_MS, 20_000 + Math.max(1, modelCount) * 60_000)
}

export async function probeChannel(id: number, modelName?: string, all = false, modelCount = 1): Promise<boolean> {
  try {
    const query = modelName ? `?model=${encodeURIComponent(modelName)}` : all ? '?all=1' : ''
    const response = await request(`/channels/${id}/probe${query}`, { method: 'POST' }, all ? probeTimeoutMs(modelCount) : PROBE_TIMEOUT_MS)
    return response.ok
  } catch {
    return false
  }
}

export async function probeChannelDetails(
  id: number,
  modelName?: string,
  options?: { all?: boolean; modelCount?: number },
): Promise<ProbeResult[] | 'busy' | 'disabled' | null> {
  try {
    const all = !modelName && Boolean(options?.all)
    const query = modelName ? `?model=${encodeURIComponent(modelName)}` : all ? '?all=1' : ''
    const timeout = all ? probeTimeoutMs(options?.modelCount) : PROBE_TIMEOUT_MS
    const response = await request(`/channels/${id}/probe${query}`, { method: 'POST' }, timeout)
    if (!response.ok) {
      const payload = (await response.json().catch(() => ({}))) as { code?: string; error?: string }
      if (response.status === 409) {
        if (payload.code === 'probe_busy') return 'busy'
        if (payload.code === 'channel_disabled') return 'disabled'
      }
      if (payload.error && modelName) {
        return [{
          model: modelName,
          status: 'error',
          message: payload.error,
        }]
      }
      return null
    }
    const payload = (await response.json()) as { data?: unknown[] }
    if (!Array.isArray(payload.data)) return []
    return payload.data.map((rawItem) => {
      const item = asRecord(rawItem)
      return {
        model: String(item.model ?? item.Model ?? ''),
        ...(typeof item.accepted === 'boolean' ? { accepted: item.accepted } : {}),
        checkedAt: String(item.checked_at ?? item.CheckedAt ?? ''),
        status: String(item.status ?? item.Status ?? 'unknown'),
        latencyMs: numberOrNull(item.latency_ms ?? item.LatencyMs),
        ttftMs: numberOrNull(item.ttft_ms ?? item.TTFTMs),
        pingLatencyMs: numberOrNull(item.ping_latency_ms ?? item.PingLatencyMs),
        message: String(item.message ?? item.Message ?? ''),
      }
    })
  } catch {
    return null
  }
}

export type ModelInjectionResult = {
  model: string
  accepted?: boolean
  state: 'clean' | 'suspected' | 'detected' | 'error'
  prompt_tokens: number
  content?: string
  message?: string
  checked_at: string
}

// Checks the first configured model, or every model the channel lists when all is true.
// The call is synchronous and a full sweep runs one request per model (plus an
// extraction for injected ones), hence the long deadlines.
export async function injectionCheck(
  id: number,
  options?: boolean | { all?: boolean; model?: string }
): Promise<ModelInjectionResult[] | null> {
  try {
    let query = ''
    let isAll = false
    if (typeof options === 'boolean') {
      isAll = options
      query = options ? '?all=1' : ''
    } else if (options) {
      if (options.model) {
        query = `?model=${encodeURIComponent(options.model)}`
      } else if (options.all) {
        isAll = true
        query = '?all=1'
      }
    }
    const timeoutMs = isAll ? 540_000 : 70_000
    const response = await request(`/channels/${id}/injection-check${query}`, { method: 'POST' }, timeoutMs)
    if (!response.ok) return null
    const payload = (await response.json()) as { data?: { results?: ModelInjectionResult[] } }
    return payload.data?.results ?? []
  } catch {
    return null
  }
}

export async function duplicateChannel(id: number): Promise<ApiChannel | null> {
  try {
    const response = await request(`/channels/${id}/duplicate`, { method: 'POST' })
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord }
    return payload.data ? normalizeChannel(payload.data) : null
  } catch {
    return null
  }
}

export type TraceStep = {
  key_id?: number
  key_name?: string
  index: number
  channel_name: string
  channel_id: number
  endpoint: string
  model: string
  attempt: number
  status_code: number
  latency_ms: number
  ttft_ms?: number
  error?: string
  raw_error?: string
  succeeded: boolean
}

export type RequestLog = {
  id: number
  created_at: string
  model: string
  channel_id: number
  channel_name: string
  status_code: number
  latency_ms: number
  ttft_ms?: number
  prompt_tokens?: number
  completion_tokens?: number
  total_tokens?: number
  is_stream?: boolean
  is_failover: boolean
  failover_from: string
  error_message: string
  upstream_model?: string
  endpoint?: string
  client_ip?: string
  user_agent?: string
  request_path?: string
  finish_reason?: string
  trace_json?: string
}

export async function listLogs(limit = 100): Promise<RequestLog[] | null> {
  try {
    const response = await request(`/logs?limit=${limit}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: RequestLog[] }
    return Array.isArray(payload.data) ? payload.data : []
  } catch {
    return null
  }
}

export type LogEventCallback = {
  onCreated?: (log: RequestLog) => void
  onUpdated?: (log: RequestLog) => void
  onCleared?: () => void
  onPruned?: () => void
  onConnected?: () => void
  onError?: (err: Event) => void
}

export function subscribeLogs(callbacks: LogEventCallback): () => void {
  const url = `${endpoint}/logs/stream`
  let eventSource: EventSource | null = new EventSource(url)

  eventSource.addEventListener('connected', () => {
    callbacks.onConnected?.()
  })

  eventSource.addEventListener('log_created', (evt: MessageEvent) => {
    try {
      const log = JSON.parse(evt.data) as RequestLog
      callbacks.onCreated?.(log)
    } catch {
      // Ignore malformed events and keep the live stream connected.
    }
  })

  eventSource.addEventListener('log_updated', (evt: MessageEvent) => {
    try {
      const log = JSON.parse(evt.data) as RequestLog
      callbacks.onUpdated?.(log)
    } catch {
      // Ignore malformed events and keep the live stream connected.
    }
  })

  eventSource.addEventListener('logs_cleared', () => {
    callbacks.onCleared?.()
  })

  eventSource.addEventListener('logs_pruned', () => {
    callbacks.onPruned?.()
  })

  eventSource.onerror = (err) => {
    callbacks.onError?.(err)
  }

  return () => {
    if (eventSource) {
      eventSource.close()
      eventSource = null
    }
  }
}

export async function clearLogs(): Promise<boolean> {
  try {
    const response = await request('/logs', { method: 'DELETE' })
    return response.ok
  } catch {
    return false
  }
}

export async function pruneLogs(options?: { days?: number; max_count?: number } | number): Promise<{ deleted: number } | null> {
  try {
    let body = {}
    if (typeof options === 'number') {
      body = { days: options }
    } else if (options) {
      body = options
    }
    const response = await request('/logs/prune', {
      method: 'POST',
      body: JSON.stringify(body)
    })
    if (!response.ok) return null
    const payload = await response.json() as { deleted?: number }
    return { deleted: Number(payload.deleted ?? 0) }
  } catch {
    return null
  }
}

export type SystemInfo = {
  data_dir: string
  db_path: string
  db_size_bytes: number
  log_retention_days: number
  log_max_count: number
  logs_count: number
  routing_strategy: 'smart_quality' | 'round_robin' | 'latency'
  hide_dock_icon?: boolean
  dock_supported?: boolean
}

export async function getSystemInfo(): Promise<SystemInfo | null> {
  try {
    const response = await request('/system')
    if (!response.ok) return null
    const payload = await response.json() as { data?: SystemInfo }
    return payload.data ?? null
  } catch {
    return null
  }
}

export type GatewayConnectionInfo = {
  base_url: string
  api_key: string
}

export async function getGatewayConnection(): Promise<GatewayConnectionInfo | null> {
  try {
    const response = await request('/gateway/connection')
    if (!response.ok) return null
    const payload = await response.json() as { data?: GatewayConnectionInfo }
    return payload.data ?? null
  } catch {
    return null
  }
}

export async function updateSettings(settings: {
  log_retention_days?: number
  log_max_count?: number
  routing_strategy?: 'smart_quality' | 'round_robin' | 'latency'
  hide_dock_icon?: boolean
}): Promise<boolean> {
  try {
    const response = await request('/settings', {
      method: 'PUT',
      body: JSON.stringify(settings)
    })
    return response.ok
  } catch {
    return false
  }
}

export async function fetchUpstreamModels(
  endpointUrl: string,
  apiKey: string,
  provider?: string,
  channelId?: number,
): Promise<{ ok: boolean; models?: string[]; error?: string }> {
  try {
    const response = await request('/channels/upstream-models', {
      method: 'POST',
      body: JSON.stringify({ endpoint: endpointUrl, api_key: channelId ? undefined : apiKey, provider, channel_id: channelId }),
    })
    const payload = await response.json() as { data?: string[]; error?: string }
    if (!response.ok) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return { ok: true, models: payload.data ?? [] }
  } catch (err: unknown) {
    return { ok: false, error: err instanceof Error ? err.message : '无法连接到本地网关服务' }
  }
}

export type DetectProtocolResult = {
  ok: boolean
  detectedProtocol?: 'openai-compatible' | 'anthropic' | 'openai-responses'
  supportsOpenai?: boolean | null
  supportsAnthropic?: boolean | null
  supportsResponses?: boolean | null
  models?: string[]
  message?: string
  error?: string
}

export async function detectChannelProtocol(
  endpointUrl: string,
  apiKey: string,
  channelId?: number,
  model?: string,
): Promise<DetectProtocolResult> {
  try {
    const response = await request('/channels/detect-protocol', {
      method: 'POST',
      body: JSON.stringify({
        endpoint: endpointUrl,
        api_key: apiKey || undefined,
        channel_id: channelId,
        model,
      }),
    })
    const payload = (await response.json()) as {
      data?: {
        detected_protocol: string | null
        supports_openai: boolean | null
        supports_anthropic: boolean | null
        supports_responses?: boolean | null
        models: string[]
        message: string
      }
      error?: string
    }
    if (!response.ok || !payload.data) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return {
      ok: true,
      detectedProtocol: payload.data.detected_protocol as DetectProtocolResult['detectedProtocol'] ?? undefined,
      supportsOpenai: payload.data.supports_openai,
      supportsAnthropic: payload.data.supports_anthropic,
      supportsResponses: payload.data.supports_responses,
      models: payload.data.models,
      message: payload.data.message,
    }
  } catch (err: unknown) {
    return { ok: false, error: err instanceof Error ? err.message : '无法连接到本地网关服务' }
  }
}

function toPayload(channel: Partial<ApiChannel> & { apiKey?: string }) {
  return {
    name: channel.name,
    provider: channel.provider === 'anthropic' ? 'anthropic' : 'openai',
    api_mode: channel.provider === 'openai-responses' ? 'responses' : 'chat_completions',
    endpoint: channel.url,
    api_key: channel.apiKey,
    primary_model: channel.primaryModel || '',
    extra_models: channel.extraModels ?? [],
    enabled: channel.enabled ?? true,
  }
}

export type AnalyticsSummary = {
  total_requests: number
  success_requests: number
  failed_requests: number
  success_rate: number
  total_tokens: number
  prompt_tokens: number
  completion_tokens: number
  avg_latency_ms: number
  avg_ttft_ms: number
}

export type DailyAnalyticsTrend = {
  date: string
  total_requests: number
  success_requests: number
  failed_requests: number
  total_tokens: number
  prompt_tokens: number
  completion_tokens: number
  avg_latency_ms: number
  avg_ttft_ms: number
}

export type ModelAnalyticsStat = {
  model: string
  total_requests: number
  success_requests: number
  failed_requests: number
  success_rate: number
  total_tokens: number
  prompt_tokens: number
  completion_tokens: number
  avg_latency_ms: number
  min_latency_ms: number
  max_latency_ms: number
  avg_ttft_ms: number
  top_channel: string
}

export type ChannelAnalyticsStat = {
  channel_name: string
  total_requests: number
  success_requests: number
  failed_requests: number
  success_rate: number
  total_tokens: number
  avg_latency_ms: number
  avg_ttft_ms: number
}

export type AnalyticsResult = {
  days: number
  summary: AnalyticsSummary
  daily: DailyAnalyticsTrend[]
  models: ModelAnalyticsStat[]
  channels: ChannelAnalyticsStat[]
}

export async function getAnalytics(days = 7): Promise<AnalyticsResult | null> {
  try {
    const response = await request(`/analytics?days=${days}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: AnalyticsResult }
    return payload.data ?? null
  } catch {
    return null
  }
}

export type ClientPlatformBackendConfig = {
  strategy?: string
  allowed_channels?: string[]
  model?: string
  small_model?: string
  allowed_models?: string[]
  auto_enabled?: boolean
  auto_strategy?: string
  auto_models?: string[]
  apply_local?: boolean
  restart?: boolean
  launch_terminal?: boolean
}

export async function getClientConfigs(): Promise<Record<string, ClientPlatformBackendConfig>> {
  try {
    const response = await request('/clients/config')
    if (!response.ok) return {}
    const payload = await response.json() as { data?: Record<string, ClientPlatformBackendConfig> }
    return payload.data ?? {}
  } catch {
    return {}
  }
}

export interface ClientConfigUpdateResult {
  ok: boolean
  message?: string
}

export async function updateClientConfig(
  clientId: string,
  payload: ClientPlatformBackendConfig
): Promise<ClientConfigUpdateResult> {
  try {
    const response = await request(`/clients/${encodeURIComponent(clientId)}/config`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    })
    const payloadJson = (await response.json().catch(() => ({}))) as {
      data?: { status?: string; message?: string }
      error?: string
      message?: string
    }
    return {
      ok: response.ok && payloadJson.data?.status === 'ok',
      message: payloadJson.error || payloadJson.data?.message || payloadJson.message,
    }
  } catch (err: any) {
    return { ok: false, message: err?.message }
  }
}
