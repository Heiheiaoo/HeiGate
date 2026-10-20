export type ClientPlatformId = 'hermes'

export type ClientGatewayStrategy = 'manual_priority' | 'smart_quality' | 'latency' | 'round_robin'

export type ClientGatewayConfig = {
  strategy: ClientGatewayStrategy
  allowedChannels: string[]
  model: string
  fallbackChannel?: string
  allowedModels?: string[]
}

export type ClientPlatformItem = {
  id: ClientPlatformId
  name: string
  smallModel?: string
  gateway: ClientGatewayConfig
}
