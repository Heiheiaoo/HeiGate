export type DesktopApiChannel = {
  id: number
  name: string
  provider: 'anthropic' | 'openai-compatible' | 'openai-responses'
  apiMode: 'chat_completions' | 'responses'
  url: string
  apiKey?: string
  primaryModel: string
  extraModels: string[]
  enabled: boolean
  intervalSeconds: number
  priority: number
  status: 'healthy' | 'degraded' | 'offline' | 'unknown'
  latency: number | null
  pingLatency: number | null
  lastError: string
  lastCheck: string
  failureCount: number
  circuitState: 'closed' | 'open' | 'half_open'
  cooldownUntil: string | null
}

type ApiRecord = Record<string, unknown>

const endpoint = '/desktop/api'

function protocolFromRecord(record: ApiRecord): DesktopApiChannel['provider'] {
  if (record.provider === 'anthropic') return 'anthropic'
  if (record.api_mode === 'responses') return 'openai-responses'
  return 'openai-compatible'
}

function normalizeChannel(record: ApiRecord): DesktopApiChannel {
  const status = record.last_status === 'operational' ? 'healthy' : record.last_status === 'degraded' ? 'degraded' : record.last_status === 'error' || record.last_status === 'failed' ? 'offline' : 'unknown'
  return {
    id: Number(record.id),
    name: String(record.name ?? ''),
    provider: protocolFromRecord(record),
    apiMode: record.api_mode === 'responses' ? 'responses' : 'chat_completions',
    url: String(record.endpoint ?? ''),
    apiKey: typeof record.api_key === 'string' ? record.api_key : undefined,
    primaryModel: String(record.primary_model ?? ''),
    extraModels: Array.isArray(record.extra_models) ? record.extra_models.map(String) : [],
    enabled: Boolean(record.enabled),
    intervalSeconds: Number(record.interval_seconds ?? 300),
    priority: Number(record.priority ?? 1),
    status,
    latency: record.last_latency_ms == null ? null : Number(record.last_latency_ms),
    pingLatency: record.last_ping_latency_ms == null ? null : Number(record.last_ping_latency_ms),
    lastError: String(record.last_error ?? ''),
    lastCheck: record.last_checked_at ? String(record.last_checked_at) : '尚未探活',
    failureCount: Number(record.failure_count ?? 0),
    circuitState: record.circuit_state === 'open' || record.circuit_state === 'half_open' ? record.circuit_state : 'closed',
    cooldownUntil: record.cooldown_until ? String(record.cooldown_until) : null,
  }
}

async function request(path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${endpoint}${path}`, { ...init, headers: { Accept: 'application/json', ...(init?.body ? { 'Content-Type': 'application/json' } : {}), ...(init?.headers ?? {}) } })
}

export async function listDesktopChannels(includeSecrets = true): Promise<DesktopApiChannel[] | null> {
  try {
    const response = await request(`/channels${includeSecrets ? '?include_secrets=1' : ''}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord[] }
    return Array.isArray(payload.data) ? payload.data.map(normalizeChannel) : []
  } catch {
    return null
  }
}

export async function getDesktopChannel(id: number, includeSecrets = true): Promise<DesktopApiChannel | null> {
  try {
    const response = await request(`/channels/${id}${includeSecrets ? '?include_secrets=1' : ''}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord }
    return payload.data ? normalizeChannel(payload.data) : null
  } catch {
    return null
  }
}

export async function getDesktopGatewayKey(): Promise<string | null> {
  try {
    const response = await request('/session')
    if (!response.ok) return null
    const payload = await response.json() as { data?: { api_key?: string } }
    return typeof payload.data?.api_key === 'string' && payload.data.api_key ? payload.data.api_key : null
  } catch {
    return null
  }
}

export async function createDesktopChannel(channel: Partial<DesktopApiChannel> & { apiKey: string }): Promise<DesktopApiChannel | null> {
  try {
    const response = await request('/channels', { method: 'POST', body: JSON.stringify(toPayload(channel)) })
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord }
    return payload.data ? normalizeChannel(payload.data) : null
  } catch {
    return null
  }
}

export async function updateDesktopChannel(channel: DesktopApiChannel): Promise<DesktopApiChannel | null> {
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

export async function deleteDesktopChannel(id: number): Promise<boolean> {
  try {
    const response = await request(`/channels/${id}`, { method: 'DELETE' })
    return response.ok
  } catch {
    return false
  }
}

export type DesktopProbeResult = {
  model: string
  status: string
  latencyMs?: number | null
  pingLatencyMs?: number | null
  message?: string
}

export async function probeDesktopChannel(id: number, modelName?: string): Promise<boolean> {
  try {
    const query = modelName ? `?model=${encodeURIComponent(modelName)}` : ''
    const response = await request(`/channels/${id}/probe${query}`, { method: 'POST' })
    return response.ok
  } catch {
    return false
  }
}

export async function probeDesktopChannelDetails(id: number, modelName?: string): Promise<DesktopProbeResult[] | null> {
  try {
    const query = modelName ? `?model=${encodeURIComponent(modelName)}` : ''
    const response = await request(`/channels/${id}/probe${query}`, { method: 'POST' })
    if (!response.ok) return null
    const payload = (await response.json()) as { data?: any[] }
    if (!Array.isArray(payload.data)) return []
    return payload.data.map((item) => ({
      model: item.model || item.Model || '',
      status: item.status || item.Status || 'unknown',
      latencyMs: item.latency_ms ?? item.LatencyMs ?? null,
      pingLatencyMs: item.ping_latency_ms ?? item.PingLatencyMs ?? null,
      message: item.message || item.Message || '',
    }))
  } catch {
    return null
  }
}

export async function duplicateDesktopChannel(id: number): Promise<DesktopApiChannel | null> {
  try {
    const response = await request(`/channels/${id}/duplicate`, { method: 'POST' })
    if (!response.ok) return null
    const payload = await response.json() as { data?: ApiRecord }
    return payload.data ? normalizeChannel(payload.data) : null
  } catch {
    return null
  }
}

export type DesktopTraceStep = {
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

export type DesktopRequestLog = {
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

export async function listDesktopLogs(limit = 100): Promise<DesktopRequestLog[] | null> {
  try {
    const response = await request(`/logs?limit=${limit}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: DesktopRequestLog[] }
    return Array.isArray(payload.data) ? payload.data : []
  } catch {
    return null
  }
}

export type DesktopLogEventCallback = {
  onCreated?: (log: DesktopRequestLog) => void
  onUpdated?: (log: DesktopRequestLog) => void
  onCleared?: () => void
  onPruned?: () => void
  onConnected?: () => void
  onError?: (err: any) => void
}

export function subscribeDesktopLogs(callbacks: DesktopLogEventCallback): () => void {
  const url = `${endpoint}/logs/stream`
  let eventSource: EventSource | null = new EventSource(url)

  eventSource.addEventListener('connected', () => {
    callbacks.onConnected?.()
  })

  eventSource.addEventListener('log_created', (evt: MessageEvent) => {
    try {
      const log = JSON.parse(evt.data) as DesktopRequestLog
      callbacks.onCreated?.(log)
    } catch {
      // Ignore malformed events and keep the live stream connected.
    }
  })

  eventSource.addEventListener('log_updated', (evt: MessageEvent) => {
    try {
      const log = JSON.parse(evt.data) as DesktopRequestLog
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

export async function clearDesktopLogs(): Promise<boolean> {
  try {
    const response = await request('/logs', { method: 'DELETE' })
    return response.ok
  } catch {
    return false
  }
}

export async function pruneDesktopLogs(options?: { days?: number; max_count?: number } | number): Promise<{ deleted: number } | null> {
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

export type DesktopSystemInfo = {
  data_dir: string
  db_path: string
  db_size_bytes: number
  log_retention_days: number
  log_max_count: number
  logs_count: number
  routing_strategy: 'smart_quality' | 'priority' | 'round_robin' | 'latency'
}

export async function getDesktopSystemInfo(): Promise<DesktopSystemInfo | null> {
  try {
    const response = await request('/system')
    if (!response.ok) return null
    const payload = await response.json() as { data?: DesktopSystemInfo }
    return payload.data ?? null
  } catch {
    return null
  }
}

export async function updateDesktopSettings(settings: {
  log_retention_days?: number
  log_max_count?: number
  routing_strategy?: 'smart_quality' | 'priority' | 'round_robin' | 'latency'
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

export async function fetchDesktopUpstreamModels(
  endpointUrl: string,
  apiKey: string,
  provider?: string,
): Promise<{ ok: boolean; models?: string[]; error?: string }> {
  try {
    const response = await request('/channels/upstream-models', {
      method: 'POST',
      body: JSON.stringify({ endpoint: endpointUrl, api_key: apiKey, provider }),
    })
    const payload = await response.json() as { data?: string[]; error?: string }
    if (!response.ok) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return { ok: true, models: payload.data ?? [] }
  } catch (err: any) {
    return { ok: false, error: err?.message || '无法连接到本地网关服务' }
  }
}

export type ClaudeModelMappingItem = {
  desktop_slot: string
  upstream_model: string
  label_override?: string
  supports_1m?: boolean
}

export type DesktopClientStatus = {
  type: 'claude' | 'codex'
  name: string
  installed: boolean
  app_path: string
  running: boolean
  configured: boolean
  gateway_url: string
  config_path: string
  configured_models: string[]
  default_model?: string
  mappings?: ClaudeModelMappingItem[]
}

export async function listDesktopClients(): Promise<DesktopClientStatus[] | null> {
  try {
    const response = await request('/clients')
    if (!response.ok) return null
    const payload = await response.json() as { data?: DesktopClientStatus[] }
    return Array.isArray(payload.data) ? payload.data : []
  } catch {
    return null
  }
}

export async function configureDesktopClaude(params: {
  models?: string[]
  mappings?: ClaudeModelMappingItem[]
}): Promise<{ ok: boolean; message?: string; error?: string }> {
  try {
    const response = await request('/clients/claude/configure', {
      method: 'POST',
      body: JSON.stringify(params),
    })
    const payload = await response.json() as { message?: string; error?: string }
    if (!response.ok) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return { ok: true, message: payload.message }
  } catch (err: any) {
    return { ok: false, error: err?.message || '无法连接到本地网关' }
  }
}

export async function launchDesktopClaude(): Promise<{ ok: boolean; message?: string; error?: string }> {
  try {
    const response = await request('/clients/claude/launch', {
      method: 'POST',
    })
    const payload = await response.json() as { message?: string; error?: string }
    if (!response.ok) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return { ok: true, message: payload.message }
  } catch (err: any) {
    return { ok: false, error: err?.message || '无法启动 Claude 桌面端' }
  }
}

export async function configureDesktopCodex(params: {
  models: string[]
  default_model?: string
}): Promise<{ ok: boolean; message?: string; error?: string }> {
  try {
    const response = await request('/clients/codex/configure', {
      method: 'POST',
      body: JSON.stringify(params),
    })
    const payload = await response.json() as { message?: string; error?: string }
    if (!response.ok) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return { ok: true, message: payload.message }
  } catch (err: any) {
    return { ok: false, error: err?.message || '无法连接到本地网关' }
  }
}

export async function launchDesktopCodex(): Promise<{ ok: boolean; message?: string; error?: string }> {
  try {
    const response = await request('/clients/codex/launch', {
      method: 'POST',
    })
    const payload = await response.json() as { message?: string; error?: string }
    if (!response.ok) {
      return { ok: false, error: payload.error || `HTTP ${response.status}` }
    }
    return { ok: true, message: payload.message }
  } catch (err: any) {
    return { ok: false, error: err?.message || '无法启动 Codex 桌面端' }
  }
}

function toPayload(channel: Partial<DesktopApiChannel> & { apiKey?: string }) {
  return {
    name: channel.name,
    provider: channel.provider === 'anthropic' ? 'anthropic' : 'openai',
    api_mode: channel.provider === 'openai-responses' ? 'responses' : 'chat_completions',
    endpoint: channel.url,
    api_key: channel.apiKey,
    primary_model: channel.primaryModel || '',
    extra_models: channel.extraModels ?? [],
    enabled: channel.enabled ?? true,
    interval_seconds: channel.intervalSeconds ?? 300,
    priority: channel.priority ?? 1,
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

export type DesktopAnalyticsResult = {
  days: number
  summary: AnalyticsSummary
  daily: DailyAnalyticsTrend[]
  models: ModelAnalyticsStat[]
  channels: ChannelAnalyticsStat[]
}

export async function getDesktopAnalytics(days = 7): Promise<DesktopAnalyticsResult | null> {
  try {
    const response = await request(`/analytics?days=${days}`)
    if (!response.ok) return null
    const payload = await response.json() as { data?: DesktopAnalyticsResult }
    return payload.data ?? null
  } catch {
    return null
  }
}

export async function getCustomModelMappings(): Promise<Record<string, string>> {
  try {
    const response = await request('/custom-model-mappings')
    if (!response.ok) return {}
    const payload = await response.json() as { data?: Record<string, string> }
    return payload.data ?? {}
  } catch {
    return {}
  }
}

export async function setCustomModelMapping(rawModel: string, targetModel: string): Promise<Record<string, string> | null> {
  try {
    const response = await request('/custom-model-mappings', {
      method: 'POST',
      body: JSON.stringify({ raw_model: rawModel, target_model: targetModel }),
    })
    if (!response.ok) return null
    const payload = await response.json() as { data?: Record<string, string> }
    return payload.data ?? null
  } catch {
    return null
  }
}

export async function deleteCustomModelMapping(rawModel: string): Promise<Record<string, string> | null> {
  try {
    const response = await request(`/custom-model-mappings?raw_model=${encodeURIComponent(rawModel)}`, {
      method: 'DELETE',
    })
    if (!response.ok) return null
    const payload = await response.json() as { data?: Record<string, string> }
    return payload.data ?? null
  } catch {
    return null
  }
}
