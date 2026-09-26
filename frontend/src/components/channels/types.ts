export type ChannelProtocol = 'anthropic' | 'openai-compatible' | 'openai-responses'

export type ChannelStatus = 'healthy' | 'degraded' | 'offline' | 'unknown'

export type ChannelCircuitState = 'closed' | 'open' | 'half_open'

export type ChannelInjectionState = 'unknown' | 'clean' | 'suspected' | 'detected' | 'error'

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
}

export type ChannelViewMode = 'grid' | 'list'

export type ChannelFilterStatus = 'all' | 'healthy' | 'degraded' | 'offline' | 'disabled'
