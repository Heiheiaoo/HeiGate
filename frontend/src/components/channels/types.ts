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
  probeInterval: number
  probeStreak?: number
  effectiveIntervalSeconds?: number
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

export function formatIntervalText(sec: number): string {
  if (sec <= 0) return '手动'
  if (sec === 300) return '5分钟'
  if (sec === 900) return '15分钟'
  if (sec === 1800) return '30分钟'
  if (sec >= 60) return `${Math.round(sec / 60)}分钟`
  return `${sec}秒`
}

export function probeModeLabel(channel: Channel): string {
  const configured = channel.probeInterval ?? 0
  const effective = channel.effectiveIntervalSeconds ?? configured
  if (configured === 0) {
    return '队列 · 手动'
  }
  const isAdaptive = effective !== configured && configured > 0
  const timeStr = formatIntervalText(effective)
  return isAdaptive ? `队列 · ${timeStr}(自适应)` : `队列 · ${timeStr}`
}

export function probeModeTooltip(channel: Channel): string {
  const configured = channel.probeInterval ?? 0
  const effective = channel.effectiveIntervalSeconds ?? configured
  if (configured === 0) {
    return '探活模式：串行单线程队列探活 · 仅手动触发（不消耗额度）'
  }
  const isAdaptive = effective !== configured && configured > 0
  if (isAdaptive) {
    return `探活模式：串行单线程队列探活 · 配置周期 ${formatIntervalText(configured)}，当前自适应拉长至 ${formatIntervalText(effective)}`
  }
  return `探活模式：串行单线程队列探活 · 每 ${formatIntervalText(configured)} 自动周期检测`
}

export function isPeriodicProbe(channel: Channel): boolean {
  return (channel.probeInterval ?? 0) > 0
}
