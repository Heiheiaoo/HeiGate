import type { Channel } from '@/components/channels/types'

export type ClientPlatformId = 'hermes' | 'codex' | 'claude-desktop' | 'claude-code' | 'generic'

export type ClientMode = 'direct' | 'gateway'

export type ClientStatus = 'running' | 'stopped' | 'unknown'

export type ConfigFileStatus = 'synced' | 'modified' | 'not_found' | 'error'

export type ClientDirectConfig = {
  channelName: string
  endpoint: string
  apiKey: string
  model: string
  smallModel?: string
}

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
  categoryLabel: string
  description: string
  configPath: string
  supportedModes: ClientMode[]
  activeMode: ClientMode
  direct: ClientDirectConfig
  gateway: ClientGatewayConfig
  status: ClientStatus
  fileStatus: ConfigFileStatus
  lastBackupTime?: string
}

export function filterChannelsForPlatform(platformId: ClientPlatformId, channels: Channel[]): Channel[] {
  if (platformId === 'claude-desktop' || platformId === 'claude-code') {
    // Claude 客户端：优先展示 Anthropic 协议供应商，或拥有 Claude 模型的供应商
    const anthropicChannels = channels.filter(c => c.protocol === 'anthropic')
    if (anthropicChannels.length > 0) return anthropicChannels
    return channels.filter(c => (c.modelNames || []).some(m => m.toLowerCase().includes('claude')))
  }
  if (platformId === 'codex') {
    // 一个上游可能同时实现 Chat 与 Responses；protocol 是网关当前选用
    // 的模式，supportsResponses 才表达该地址实际提供的全部协议能力。
    const responses = channels.filter(c => c.enabled && (c.protocol === 'openai-responses' || c.supportsResponses === true))
    if (responses.length > 0) return responses
    return channels.filter(c => c.enabled && c.protocol === 'openai-compatible')
  }
  // Hermes 默认走网关，可使用全部可用供应商
  return channels
}
