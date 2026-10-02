export type ChannelProtocol = 'anthropic' | 'openai-compatible' | 'openai-responses'

export type ChannelStatus = 'healthy' | 'degraded' | 'offline' | 'unknown'

export type ChannelCircuitState = 'closed' | 'open' | 'half_open'

export type ChannelInjectionState = 'unknown' | 'clean' | 'suspected' | 'detected' | 'error'

export type ModelHealth = {
  model: string
  status: string
  latencyMs: number | null
  ttftMs: number | null
  error: string
  checkedAt: string | null
}

export type ModelLatencyRow = {
  name: string
  selected: boolean
  checkedAtLabel: string
  latencyClass: string
  latencyDisplay: string
  latencyTooltip: string
  failed: boolean
  injectionClass: string
  injectionTooltip: string
  injectionLabel: string
  primary: boolean
  probing: boolean
  checkingInjection: boolean
  copied: boolean
}

export type Channel = {
  name: string
  url: string
  protocol: ChannelProtocol
  status: ChannelStatus
  latency: number | null
  ttft: number | null
  models: number
  lastCheck: string
  accent: string
  enabled: boolean
  apiKey?: string
  primaryModel?: string
  modelNames?: string[]
  probing?: boolean
  gatewayId?: number
  failureCount?: number
  circuitState?: ChannelCircuitState
  cooldownUntil?: string | null
  lastError?: string
  injectionState?: ChannelInjectionState
  injectionTokens?: number | null
  injectionContent?: string
  injectionCheckedAt?: string | null
  modelHealth?: ModelHealth[]
}

export type ChannelViewMode = 'grid' | 'list'

export type ChannelFilterStatus = 'all' | 'healthy' | 'degraded' | 'offline' | 'disabled'
