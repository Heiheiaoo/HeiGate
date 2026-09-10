<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { clearDesktopValue, readDesktopValue, writeDesktopValue } from '@/services/desktopStorage'
import { checkLocalGateway } from '@/services/localGateway'
import {
  clearDesktopLogs,
  createDesktopChannel,
  deleteDesktopChannel,
  duplicateDesktopChannel,
  fetchDesktopUpstreamModels,
  getDesktopChannel,
  getDesktopGatewayKey,
  getDesktopSystemInfo,
  listDesktopChannels,
  listDesktopLogs,
  probeDesktopChannelDetails,
  pruneDesktopLogs,
  subscribeDesktopLogs,
  updateDesktopChannel,
  updateDesktopSettings,
  listDesktopClients,
  configureDesktopClaude,
  launchDesktopClaude,
  configureDesktopCodex,
  launchDesktopCodex,
  getDesktopAnalytics,
  getCustomModelMappings,
  setCustomModelMapping,
  deleteCustomModelMapping,
  type DesktopApiChannel,
  type DesktopRequestLog,
  type DesktopTraceStep,
  type DesktopSystemInfo,
  type DesktopClientStatus,
  type DesktopAnalyticsResult,
} from '@/services/desktopApi'

export type Channel = {
  name: string
  url: string
  protocol: 'anthropic' | 'openai-compatible' | 'openai-responses'
  status: 'healthy' | 'degraded' | 'offline' | 'unknown'
  latency: number | null
  models: number
  priority: number
  lastCheck: string
  accent: string
  enabled: boolean
  probeInterval: number
  apiKey?: string
  primaryModel?: string
  modelNames?: string[]
  probing?: boolean
  desktopId?: number
  failureCount?: number
  circuitState?: 'closed' | 'open' | 'half_open'
  cooldownUntil?: string | null
  lastError?: string
}

export type GlobalRouteStrategy = 'smart_quality' | 'latency' | 'round_robin' | 'priority'

const isDesktop = ref(typeof window !== 'undefined' && Boolean((window as any).webkit?.messageHandlers?.dragWindow))
const savedSection = typeof localStorage !== 'undefined' ? (localStorage.getItem('heigate_active_section') as any) : null
const activeSection = ref<'overview' | 'analytics' | 'channels' | 'models' | 'logs' | 'settings'>(
  savedSection && ['overview', 'analytics', 'channels', 'models', 'logs', 'settings'].includes(savedSection) ? savedSection : 'logs'
)
watch(activeSection, async (val) => {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('heigate_active_section', val)
  }
  if (val === 'overview' && !gatewayApiKey.value) {
    gatewayApiKey.value = (await getDesktopGatewayKey()) ?? ''
  }
  if (val === 'analytics') {
    void loadAnalytics()
  }
})
const navCenterRef = ref<HTMLElement | null>(null)
const isCompactTabs = ref(typeof window !== 'undefined' && window.innerWidth < 750)
let navCenterObserver: ResizeObserver | null = null

function checkTabsFit() {
  if (navCenterRef.value) {
    isCompactTabs.value = navCenterRef.value.clientWidth < 480
  } else if (typeof window !== 'undefined') {
    isCompactTabs.value = window.innerWidth < 750
  }
}
const showAddChannel = ref(false)
const showEditChannel = ref(false)
const editingChannel = ref<Channel | null>(null)
const showApiKey = ref(false)
const channelSearch = ref('')
const searchInputRef = ref<HTMLInputElement | null>(null)
const protocolFilter = ref<'all' | 'anthropic' | 'openai-compatible' | 'openai-responses'>('all')
const routingStrategy = ref<GlobalRouteStrategy>('latency')

// Confirm Modal state for robust in-app confirmation (avoids WKWebView confirm suppression)
const confirmModal = ref<{
  title: string
  message: string
  description?: string
  danger?: boolean
  confirmText?: string
  onConfirm: () => Promise<void> | void
} | null>(null)

// Model Latency & Realtime Speed Test Modal
const activeModelLatencyChannel = ref<Channel | null>(null)
const modelSearchQuery = ref('')
const modelLatencies = ref<Record<string, {
  latency: number | null
  probing?: boolean
  status?: string
  error?: string | null
  message?: string
}>>({})
const isProbingAllModels = ref(false)

// Model Multi-Provider Speed Test Modal
const activeModelSpeedTestName = ref<string | null>(null)
const isProbingModelProviders = ref(false)
const modelSpeedTestQuery = ref('')



function getChannelModels(c: Channel): string[] {
  if (c.modelNames && c.modelNames.length > 0) {
    return c.modelNames
  }
  return ['默认模型']
}

// Analytics state & helpers
const analyticsDays = ref(7)
const analyticsData = ref<DesktopAnalyticsResult | null>(null)
const isLoadingAnalytics = ref(false)
const analyticsModelSearch = ref('')

async function loadAnalytics() {
  isLoadingAnalytics.value = true
  try {
    const data = await getDesktopAnalytics(analyticsDays.value)
    if (data) {
      analyticsData.value = data
    }
  } finally {
    isLoadingAnalytics.value = false
  }
}

function setAnalyticsDays(days: number) {
  analyticsDays.value = days
  void loadAnalytics()
}

function formatAnalyticsTokens(val: number): string {
  if (!val || isNaN(val)) return '0'
  if (val >= 1000000) return (val / 1000000).toFixed(2) + 'M'
  if (val >= 1000) return (val / 1000).toFixed(1) + 'K'
  return val.toLocaleString()
}

function formatAnalyticsLatency(ms: number): string {
  if (!ms || isNaN(ms) || ms <= 0) return '-'
  if (ms >= 1000) return (ms / 1000).toFixed(2) + 's'
  return Math.round(ms) + 'ms'
}

function getLatencyTagClass(ms: number): string {
  if (!ms || ms <= 0) return 'text-muted'
  if (ms < 1000) return 'm26-tag-fast'
  if (ms < 3000) return 'm26-tag-normal'
  return 'm26-tag-slow'
}

const filteredAnalyticsModels = computed(() => {
  if (!analyticsData.value?.models) return []
  const query = analyticsModelSearch.value.trim().toLowerCase()
  if (!query) return analyticsData.value.models
  return analyticsData.value.models.filter(m => m.model.toLowerCase().includes(query) || (m.top_channel && m.top_channel.toLowerCase().includes(query)))
})

const maxDailyRequests = computed(() => {
  if (!analyticsData.value?.daily?.length) return 1
  return Math.max(...analyticsData.value.daily.map(d => d.total_requests), 1)
})

const maxDailyTokens = computed(() => {
  if (!analyticsData.value?.daily?.length) return 1
  return Math.max(...analyticsData.value.daily.map(d => d.total_tokens), 1)
})

// Channel View Mode (Cards Grid vs Table List)
const channelViewMode = ref<'grid' | 'list'>('list')
function setChannelViewMode(mode: 'grid' | 'list') {
  channelViewMode.value = mode
  try {
    localStorage.setItem('heigate_channel_view_mode', mode)
  } catch (err) {
    console.debug('Failed to persist view mode', err)
  }
}

// Interval Preset Options (0s = Manual Only, saves tokens)
const intervalPresets = [
  { label: '仅手动 (不耗Token)', value: 0 },
  { label: '5分钟', value: 300 },
  { label: '15分钟', value: 900 },
  { label: '30分钟', value: 1800 },
  { label: '自定义', value: -1 },
]

function getIntervalDisplay(interval: number): string {
  if (interval <= 0) return '手动'
  if (interval === 300) return '5分钟'
  if (interval === 900) return '15分钟'
  if (interval === 1800) return '30分钟'
  if (interval >= 60) return `${Math.round(interval / 60)}分钟`
  return `${interval}秒`
}



// Add Channel Form State
const newChannelName = ref('')
const newChannelUrl = ref('')
const newChannelKey = ref('')
const newChannelProtocol = ref<Channel['protocol']>('openai-compatible')
const newChannelEnabled = ref(true)
const newChannelProbePreset = ref<number>(0)
const newChannelProbeCustom = ref<number>(60)
const newChannelModelText = ref('gpt-4o-mini')
const isFetchingNewModels = ref(false)
const showNewKey = ref(false)

// Edit Channel Form State
const editChannelName = ref('')
const editChannelUrl = ref('')
const editChannelKey = ref('')
const editChannelProtocol = ref<Channel['protocol']>('openai-compatible')
const editChannelEnabled = ref(true)
const editChannelProbePreset = ref<number>(0)
const editChannelProbeCustom = ref<number>(60)
const editChannelPriority = ref<number>(1)
const editChannelModelText = ref('')
const isFetchingEditModels = ref(false)
const showEditKey = ref(false)

const configInput = ref<HTMLInputElement | null>(null)
const desktopApiActive = ref(false)
const gatewayOnline = ref(false)
const gatewayLatency = ref<number | null>(null)
const gatewayLastChecked = ref('尚未检测')
const gatewayApiKey = ref('')
const showOverviewApiKey = ref(false)
const maskedOverviewApiKey = computed(() => {
  const k = gatewayApiKey.value
  if (!k) return '••••••••'
  if (k.length <= 16) return '••••••••'
  return `${k.slice(0, 8)}••••••••${k.slice(-6)}`
})
const isTestingGateway = ref(false)

// System Info & Storage & Logs
const systemInfo = ref<DesktopSystemInfo | null>(null)
const logRetentionDays = ref<number>(7)
const logMaxCount = ref<number>(5000)
const isPruningLogs = ref(false)
const isClearingLogs = ref(false)
const desktopLogs = ref<DesktopRequestLog[]>([])
const activeLogDetail = ref<DesktopRequestLog | null>(null)
const logsStreamConnected = ref(false)
const autoScrollLogs = ref(true)
let logsUnsubscribe: (() => void) | null = null
const logsTableWrapRef = ref<HTMLDivElement | null>(null)
const copiedModelId = ref('')

const copiedTarget = ref<'url' | 'key' | ''>('')
let copiedTimer: number | undefined

function markCopied(target: 'url' | 'key') {
  copiedTarget.value = target
  if (copiedTimer) window.clearTimeout(copiedTimer)
  copiedTimer = window.setTimeout(() => {
    copiedTarget.value = ''
  }, 2000)
}

const toastMessage = ref('')
let toastTimer: number | undefined
function showToast(msg: string) {
  toastMessage.value = msg
  if (toastTimer) window.clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => {
    toastMessage.value = ''
  }, 2400)
}

const gatewayBaseUrl = computed(() => {
  if (typeof window === 'undefined') return 'http://127.0.0.1:8080/v1'
  if (window.location.port === '5173' || window.location.port === '3000') {
    return 'http://127.0.0.1:8080/v1'
  }
  return `${window.location.origin}/v1`
})

const gatewayHostOnly = computed(() => {
  try {
    const parsed = new URL(gatewayBaseUrl.value)
    return `${parsed.hostname}:${parsed.port || (parsed.protocol === 'https:' ? '443' : '80')}`
  } catch {
    return '127.0.0.1:8080'
  }
})


type RequestEvent = {
  time: string
  model: string
  channel: string
  result: string
  detail: string
  tone: string
}

const channels = ref<Channel[]>([])
const events = ref<RequestEvent[]>([])

function channelRenderKey(channel: Channel, index: number): string {
  return channel.desktopId ? `desktop-${channel.desktopId}` : `local-${channel.name}-${channel.url}-${index}`
}

const filteredChannels = computed(() => {
  const query = channelSearch.value.trim().toLowerCase()
  return channels.value.filter((channel) => {
    const matchesQuery = !query || `${channel.name} ${channel.url}`.toLowerCase().includes(query)
    const matchesProtocol = protocolFilter.value === 'all' || channel.protocol === protocolFilter.value
    return matchesQuery && matchesProtocol
  })
})

// --- 客户端集成 (Claude 桌面端 & Codex 桌面端) ---
const desktopClients = ref<DesktopClientStatus[]>([])
const isLoadingClients = ref(false)
const showClaudeConfigModal = ref(false)
const showCodexConfigModal = ref(false)

// Claude Slots and Mappings State
const claudeSelectedModels = ref<string[]>([])
const claudeMappings = ref<{
  slot: string
  slotLabel: string
  upstreamModel: string
  supports1m: boolean
}[]>([
  { slot: 'claude-sonnet-4-6', slotLabel: 'Sonnet 4.6 (主力推荐)', upstreamModel: '', supports1m: true },
  { slot: 'claude-opus-4-8', slotLabel: 'Opus 4.8 (超强推理)', upstreamModel: '', supports1m: true },
  { slot: 'claude-haiku-4-5', slotLabel: 'Haiku 4.5 (极速响应)', upstreamModel: '', supports1m: true },
  { slot: 'claude-fable-5', slotLabel: 'Fable 5 (旗舰全能)', upstreamModel: '', supports1m: true },
  { slot: 'claude-opus-4-6', slotLabel: 'Opus 4.6', upstreamModel: '', supports1m: true },
  { slot: 'claude-opus-4-7', slotLabel: 'Opus 4.7', upstreamModel: '', supports1m: true },
  { slot: 'claude-sonnet-4-6-r2', slotLabel: 'Sonnet 4.6 R2', upstreamModel: '', supports1m: true },
])
const isSavingClaudeConfig = ref(false)
const isLaunchingClaude = ref(false)

// Codex Config State
const codexSelectedModels = ref<string[]>([])
const codexDefaultModel = ref('')
const isSavingCodexConfig = ref(false)
const isLaunchingCodex = ref(false)

// Available models computed from all enabled channels
interface ModelHealthInfo {
  status: 'healthy' | 'degraded' | 'failed' | 'unknown'
  statusClass: string
  channelName: string
  latencyText: string
  tooltip: string
}

function getModelHealth(modelName: string): ModelHealthInfo {
  const matchingChannels = channels.value.filter((c) => c.enabled && c.modelNames?.includes(modelName))
  if (matchingChannels.length === 0) {
    return {
      status: 'unknown',
      statusClass: 'is-unknown',
      channelName: '未绑定',
      latencyText: '',
      tooltip: '未匹配到已启用的上游渠道',
    }
  }

  const healthy = matchingChannels.find((c) => c.status === 'healthy')
  if (healthy) {
    const lat = healthy.latency ? `${healthy.latency}ms` : ''
    return {
      status: 'healthy',
      statusClass: 'is-healthy',
      channelName: healthy.name,
      latencyText: lat,
      tooltip: `${healthy.name}: 运行正常 (${lat || '已验证'})`,
    }
  }

  const degraded = matchingChannels.find((c) => c.status === 'degraded')
  if (degraded) {
    return {
      status: 'degraded',
      statusClass: 'is-degraded',
      channelName: degraded.name,
      latencyText: degraded.latency ? `${degraded.latency}ms` : '',
      tooltip: `${degraded.name}: 响应延迟较高`,
    }
  }

  const failed = matchingChannels.find((c) => c.status === 'offline' || Boolean(c.lastError))
  if (failed) {
    return {
      status: 'failed',
      statusClass: 'is-failed',
      channelName: failed.name,
      latencyText: '异常',
      tooltip: `${failed.name}: ${failed.lastError || '探测异常或余额不足'}`,
    }
  }

  return {
    status: 'unknown',
    statusClass: 'is-unknown',
    channelName: matchingChannels[0].name,
    latencyText: '',
    tooltip: `${matchingChannels[0].name}: 待检测`,
  }
}

function getModelHealthScore(modelName: string): number {
  const info = getModelHealth(modelName)
  if (info.status === 'healthy') return 100
  if (info.status === 'unknown') return 50
  if (info.status === 'degraded') return 30
  return 0
}

const allAvailableModels = computed(() => {
  const set = new Set<string>()
  for (const c of channels.value) {
    if (!c.enabled) continue
    if (c.modelNames && c.modelNames.length > 0) {
      for (const m of c.modelNames) {
        if (m && m.trim()) set.add(m.trim())
      }
    }
  }
  return Array.from(set).sort((a, b) => {
    const sA = getModelHealthScore(a)
    const sB = getModelHealthScore(b)
    if (sA !== sB) return sB - sA
    return a.localeCompare(b)
  })
})

const claudeClientInfo = computed(() => desktopClients.value.find((c) => c.type === 'claude'))
const codexClientInfo = computed(() => desktopClients.value.find((c) => c.type === 'codex'))

async function loadDesktopClients() {
  isLoadingClients.value = true
  try {
    const res = await listDesktopClients()
    if (res) {
      desktopClients.value = res
      const claude = res.find((c) => c.type === 'claude')
      if (claude) {
        if (claude.configured_models && claude.configured_models.length > 0) {
          claudeSelectedModels.value = [...claude.configured_models]
        }
        if (claude.mappings && claude.mappings.length > 0) {
          for (const m of claude.mappings) {
            const found = claudeMappings.value.find((slot) => slot.slot === m.desktop_slot)
            if (found) {
              found.upstreamModel = m.upstream_model
              found.supports1m = m.supports_1m ?? true
            }
          }
        }
      }
      const codex = res.find((c) => c.type === 'codex')
      if (codex) {
        if (codex.configured_models && codex.configured_models.length > 0) {
          codexSelectedModels.value = [...codex.configured_models]
        }
        if (codex.default_model) {
          codexDefaultModel.value = codex.default_model
        }
      }
    }
  } catch (err) {
    console.debug('Failed to load clients', err)
  } finally {
    isLoadingClients.value = false
  }
}

function openClaudeConfigModal() {
  if (claudeSelectedModels.value.length === 0 && allAvailableModels.value.length > 0) {
    claudeSelectedModels.value = [...allAvailableModels.value]
  }
  autoAssignClaudeSlots()
  showClaudeConfigModal.value = true
}

function toggleClaudeModel(model: string) {
  const idx = claudeSelectedModels.value.indexOf(model)
  if (idx >= 0) {
    claudeSelectedModels.value.splice(idx, 1)
  } else {
    claudeSelectedModels.value.push(model)
  }
}

function selectAllClaudeModels() {
  claudeSelectedModels.value = [...allAvailableModels.value]
  autoAssignClaudeSlots()
}

function clearClaudeModels() {
  claudeSelectedModels.value = []
}

function autoAssignClaudeSlots() {
  const models = claudeSelectedModels.value.length > 0 ? claudeSelectedModels.value : allAvailableModels.value
  if (models.length === 0) return
  const sorted = [...models].sort((a, b) => {
    const sA = getModelHealthScore(a)
    const sB = getModelHealthScore(b)
    if (sA !== sB) return sB - sA
    return a.localeCompare(b)
  })
  claudeMappings.value.forEach((item, idx) => {
    item.upstreamModel = sorted[idx % sorted.length]
  })
}

async function handleSaveClaudeConfig(launchAfter = false) {
  isSavingClaudeConfig.value = true
  try {
    const mappings = claudeMappings.value
      .filter((m) => m.upstreamModel)
      .map((m) => ({
        desktop_slot: m.slot,
        upstream_model: m.upstreamModel,
        label_override: m.upstreamModel,
        supports_1m: m.supports1m,
      }))

    const models = claudeSelectedModels.value.length > 0 ? claudeSelectedModels.value : mappings.map((m) => m.upstream_model)
    if (models.length === 0) {
      showToast('请至少选择或配置一个模型')
      return
    }

    const res = await configureDesktopClaude({ models, mappings })
    if (res.ok) {
      await loadDesktopClients()
      showClaudeConfigModal.value = false
      if (launchAfter) {
        await handleLaunchClaude()
      } else {
        showToast(res.message || 'Claude 桌面端配置已生效！')
      }
    } else {
      showToast(res.error || '写入 Claude 配置失败')
    }
  } catch (err: any) {
    showToast(err?.message || '配置失败')
  } finally {
    isSavingClaudeConfig.value = false
  }
}

async function handleLaunchClaude() {
  isLaunchingClaude.value = true
  try {
    if (!claudeClientInfo.value?.configured || (claudeClientInfo.value?.configured_models?.length ?? 0) === 0) {
      if (allAvailableModels.value.length > 0) {
        if (claudeSelectedModels.value.length === 0) {
          claudeSelectedModels.value = [...allAvailableModels.value]
        }
        autoAssignClaudeSlots()
        await handleSaveClaudeConfig(false)
      }
    }
    const res = await launchDesktopClaude()
    if (res.ok) {
      showToast('Claude 桌面端已唤起启动！')
      await loadDesktopClients()
    } else {
      showToast(res.error || '调起 Claude 失败')
    }
  } catch (err: any) {
    showToast(err?.message || '调起失败')
  } finally {
    isLaunchingClaude.value = false
  }
}

function openCodexConfigModal() {
  if (codexSelectedModels.value.length === 0 && allAvailableModels.value.length > 0) {
    codexSelectedModels.value = [...allAvailableModels.value]
  }
  if (!codexDefaultModel.value && codexSelectedModels.value.length > 0) {
    codexDefaultModel.value = codexSelectedModels.value[0]
  }
  showCodexConfigModal.value = true
}

function toggleCodexModel(model: string) {
  const idx = codexSelectedModels.value.indexOf(model)
  if (idx >= 0) {
    codexSelectedModels.value.splice(idx, 1)
    if (codexDefaultModel.value === model) {
      codexDefaultModel.value = codexSelectedModels.value[0] || ''
    }
  } else {
    codexSelectedModels.value.push(model)
    if (!codexDefaultModel.value) {
      codexDefaultModel.value = model
    }
  }
}

function selectAllCodexModels() {
  codexSelectedModels.value = [...allAvailableModels.value]
  if (!codexDefaultModel.value && codexSelectedModels.value.length > 0) {
    codexDefaultModel.value = codexSelectedModels.value[0]
  }
}

function clearCodexModels() {
  codexSelectedModels.value = []
  codexDefaultModel.value = ''
}

async function handleSaveCodexConfig(launchAfter = false) {
  isSavingCodexConfig.value = true
  try {
    const models = codexSelectedModels.value
    if (models.length === 0) {
      showToast('请至少选择一个模型配置到 Codex')
      return
    }
    const defaultModel = codexDefaultModel.value || models[0]
    const res = await configureDesktopCodex({ models, default_model: defaultModel })
    if (res.ok) {
      await loadDesktopClients()
      showCodexConfigModal.value = false
      if (launchAfter) {
        await handleLaunchCodex()
      } else {
        showToast(res.message || 'Codex 桌面端配置已生效！')
      }
    } else {
      showToast(res.error || '写入 Codex 配置失败')
    }
  } catch (err: any) {
    showToast(err?.message || '配置失败')
  } finally {
    isSavingCodexConfig.value = false
  }
}

async function handleLaunchCodex() {
  isLaunchingCodex.value = true
  try {
    if (!codexClientInfo.value?.configured) {
      if (allAvailableModels.value.length > 0) {
        if (codexSelectedModels.value.length === 0) {
          codexSelectedModels.value = [...allAvailableModels.value]
        }
        if (!codexDefaultModel.value) {
          codexDefaultModel.value = codexSelectedModels.value[0]
        }
        await handleSaveCodexConfig(false)
      }
    }
    const res = await launchDesktopCodex()
    if (res.ok) {
      showToast('Codex 桌面端 (ChatGPT.app) 已唤起启动！')
      await loadDesktopClients()
    } else {
      showToast(res.error || '调起 Codex 失败')
    }
  } catch (err: any) {
    showToast(err?.message || '调起失败')
  } finally {
    isLaunchingCodex.value = false
  }
}


const sortedChannels = computed(() => {
  const list = [...filteredChannels.value]
  return list.sort((a, b) => {
    if (a.enabled !== b.enabled) return a.enabled ? -1 : 1
    const statusWeight = (s: Channel['status']) => (s === 'healthy' ? 0 : s === 'degraded' ? 1 : 2)
    const swA = statusWeight(a.status)
    const swB = statusWeight(b.status)
    if (swA !== swB) return swA - swB
    const latA = a.latency != null && a.latency > 0 ? a.latency : 999999
    const latB = b.latency != null && b.latency > 0 ? b.latency : 999999
    if (latA !== latB) return latA - latB
    return (a.name || '').localeCompare(b.name || '')
  })
})

const enabledChannels = computed(() => channels.value.filter((channel) => channel.enabled))

const VENDOR_PREFIX_RE = /^(z-ai|qwen|deepseek-ai|deepseek|openai|anthropic|google|mistralai|mistral|meta-llama|meta|nvidia|01-ai|baichuan-inc|minimaxai|minimax|tencent|aliyun|moonshotai|bytedance|togethercomputer)\//i
const BILLING_SUFFIX_RE = /(:free|[-_]free|[-_]trial|[-_]vip|[-_]plus|[-_]preview|[-_]latest|[-_]next)$/i
const DATE_SUFFIX_RE = /(-\d{4}-\d{2}-\d{2}|-\d{8}|-\d{4})$/i
const PUNCT_RE = /[-_.\s/:]+/g

function cleanModelNameFrontend(raw: string): string {
  let s = (raw || '').trim()
  if (!s) return ''
  if (VENDOR_PREFIX_RE.test(s)) {
    s = s.replace(VENDOR_PREFIX_RE, '')
  } else {
    const idx = s.indexOf('/')
    if (idx !== -1 && idx < s.length - 1) {
      const prefix = s.slice(0, idx).toLowerCase()
      if (!prefix.includes('gpt') && !prefix.includes('claude') && !prefix.includes('gemini')) {
        s = s.slice(idx + 1)
      }
    }
  }
  s = s.replace(BILLING_SUFFIX_RE, '')
  s = s.replace(DATE_SUFFIX_RE, '')
  return s.trim()
}

function canonicalModelKeyFrontend(raw: string): string {
  const cleaned = cleanModelNameFrontend(raw)
  return cleaned.toLowerCase().replace(PUNCT_RE, '')
}

const KNOWN_EQUIVALENCE_FRONTEND: string[][] = [
  ['glm53', 'glm53flash'],
  ['glm4', 'glm4flash', 'glm4air'],
  ['deepseekchat', 'deepseekv3'],
  ['deepseekreasoner', 'deepseekr1'],
  ['qwen38flash', 'qwen38flashnext'],
  ['claude35sonnet', 'claude35sonnet20241022'],
  ['claude35haiku', 'claude35haiku20241022'],
  ['claude37sonnet', 'claude37sonnet20250219'],
  ['claude3opus', 'claude3opus20240229'],
  ['gpt4o', 'gpt4o20240806', 'gpt4o20240513'],
  ['gpt4omini', 'gpt4omini20240718'],
]

const customModelMappings = ref<Record<string, string>>({})

async function loadCustomModelMappings() {
  try {
    const data = await getCustomModelMappings()
    customModelMappings.value = data || {}
  } catch {
    customModelMappings.value = {}
  }
}

function getCanonicalGroupKey(raw: string): string {
  const trim = (raw || '').trim()
  if (!trim) return ''
  let curr = trim
  for (let i = 0; i < 5; i++) {
    let target = customModelMappings.value[curr] || customModelMappings.value[curr.toLowerCase()]
    if (!target) {
      const cleaned = cleanModelNameFrontend(curr).toLowerCase()
      if (cleaned && cleaned !== curr.toLowerCase()) {
        target = customModelMappings.value[cleaned]
      }
    }
    if (!target) {
      const cKey = canonicalModelKeyFrontend(curr)
      if (cKey) {
        for (const [k, v] of Object.entries(customModelMappings.value)) {
          if (canonicalModelKeyFrontend(k) === cKey && v) {
            target = v
            break
          }
        }
      }
    }
    if (target && target.toLowerCase() !== curr.toLowerCase()) {
      curr = target
    } else {
      break
    }
  }
  const key = canonicalModelKeyFrontend(curr)
  if (!key) return curr
  for (const group of KNOWN_EQUIVALENCE_FRONTEND) {
    if (group.includes(key)) {
      return group[0]
    }
  }
  return key
}

function isModelEquivalentFrontend(a: string, b: string): boolean {
  if (a.toLowerCase() === b.toLowerCase()) return true
  const keyA = getCanonicalGroupKey(a)
  const keyB = getCanonicalGroupKey(b)
  return Boolean(keyA && keyB && keyA === keyB)
}

function selectCanonicalDisplayNameFrontend(models: string[]): string {
  if (!models.length) return ''
  let customTarget = ''
  for (const m of models) {
    const t = customModelMappings.value[m] || customModelMappings.value[m.toLowerCase()]
    if (t) {
      customTarget = t
      break
    }
  }
  const candidateList = [...models]
  if (customTarget && !candidateList.some(c => c.toLowerCase() === customTarget.toLowerCase())) {
    candidateList.push(customTarget)
  }

  let best = ''
  let bestScore = -999
  for (const m of candidateList) {
    let score = 0
    const lower = m.toLowerCase()
    if (customTarget && lower === customTarget.toLowerCase()) score += 60
    if (m.includes('/')) score -= 30
    if (lower.includes('-free') || lower.includes(':free') || lower.includes('_free')) score -= 40
    if (lower.includes('-preview') || lower.includes('-trial') || lower.includes('-next')) score -= 20
    if (DATE_SUFFIX_RE.test(lower)) score -= 15
    if (lower.includes('-flash') || lower.includes('-pro') || lower.includes('-turbo')) score += 10
    score -= m.length
    if (score > bestScore || !best) {
      bestScore = score
      best = m
    }
  }
  if (best) {
    if (customTarget && best.toLowerCase() === customTarget.toLowerCase()) {
      return best
    }
    const cleaned = cleanModelNameFrontend(best)
    if (cleaned && !cleaned.includes('/') && !cleaned.toLowerCase().includes('free')) {
      return cleaned
    }
    return best
  }
  return models[0]
}

interface AggregateModelCard {
  name: string
  canonicalKey: string
  rawNames: string[]
  aliases: string[]
  providerCount: number
  status: 'healthy' | 'degraded' | 'offline'
  latencies: number[]
  providers: string[]
  channelDetails: Array<{
    name: string
    status: Channel['status']
    latency: number | null
    protocol: string
    upstreamModel: string
  }>
}

const models = computed(() => {
  const groups = new Map<string, {
    rawModels: Set<string>
    providers: string[]
    channelDetails: Array<{
      name: string
      status: Channel['status']
      latency: number | null
      protocol: string
      upstreamModel: string
    }>
    latencies: number[]
    status: 'healthy' | 'degraded' | 'offline'
  }>()

  channels.value.forEach((channel) => {
    const list = channel.modelNames ?? []
    list.forEach((rawModelName) => {
      const groupKey = getCanonicalGroupKey(rawModelName)
      const current = groups.get(groupKey) ?? {
        rawModels: new Set<string>(),
        providers: [],
        channelDetails: [],
        latencies: [],
        status: 'offline',
      }
      current.rawModels.add(rawModelName)

      const modelKey = getModelKey(channel, rawModelName)
      const specificEntry = modelLatencies.value[modelKey]
      const effectiveLat = specificEntry?.latency != null ? specificEntry.latency : channel.latency
      const effectiveStatus = specificEntry?.status
        ? (specificEntry.status === 'operational' ? 'healthy' : specificEntry.status === 'degraded' ? 'degraded' : 'offline')
        : channel.status

      if (!current.providers.includes(channel.name)) {
        current.providers.push(channel.name)
        current.channelDetails.push({
          name: channel.name,
          status: effectiveStatus,
          latency: effectiveLat,
          protocol: channel.protocol,
          upstreamModel: rawModelName,
        })
      }

      if (effectiveStatus === 'healthy') current.status = 'healthy'
      else if (effectiveStatus === 'degraded' && current.status !== 'healthy') current.status = 'degraded'
      if (effectiveLat != null && effectiveLat > 0) current.latencies.push(effectiveLat)

      groups.set(groupKey, current)
    })
  })

  const list: Array<AggregateModelCard & { avgLatency: number | null; latency: string }> = []
  groups.forEach((data, groupKey) => {
    const allRaw = Array.from(data.rawModels)
    const displayName = selectCanonicalDisplayNameFrontend(allRaw)
    const aliases = allRaw.filter((r) => r !== displayName)
    const avgLat = data.latencies.length
      ? Math.round(data.latencies.reduce((sum, value) => sum + value, 0) / data.latencies.length)
      : null

    list.push({
      name: displayName,
      canonicalKey: groupKey,
      rawNames: allRaw,
      aliases,
      providerCount: data.providers.length,
      status: data.status,
      latencies: data.latencies,
      providers: data.providers,
      channelDetails: data.channelDetails,
      avgLatency: avgLat,
      latency: avgLat != null ? `${avgLat}ms` : '—',
    })
  })

  return list.sort((a, b) => {
    const statusWeight = (s: 'healthy' | 'degraded' | 'offline') => (s === 'healthy' ? 0 : s === 'degraded' ? 1 : 2)
    const swA = statusWeight(a.status)
    const swB = statusWeight(b.status)
    if (swA !== swB) return swA - swB
    const latA = a.avgLatency != null && a.avgLatency > 0 ? a.avgLatency : 999999
    const latB = b.avgLatency != null && b.avgLatency > 0 ? b.avgLatency : 999999
    if (latA !== latB) return latA - latB
    return a.name.localeCompare(b.name)
  })
})

const selectedModelFamily = ref('all')

// Model Aliases Detail Modal state
const activeAliasModalModel = ref<(AggregateModelCard & { avgLatency: number | null; latency: string }) | null>(null)
const aliasModalSearch = ref('')
const copiedAliasText = ref('')

function openAliasModal(model: AggregateModelCard & { avgLatency: number | null; latency: string }) {
  activeAliasModalModel.value = model
  aliasModalSearch.value = ''
  copiedAliasText.value = ''
}

function closeAliasModal() {
  activeAliasModalModel.value = null
}

// All raw models across channels
const allDetectedChannelModels = computed(() => {
  const set = new Set<string>()
  channels.value.forEach((ch) => {
    if (ch.primaryModel) set.add(ch.primaryModel)
    if (ch.modelNames) ch.modelNames.forEach((m) => set.add(m))
  })
  return Array.from(set).sort()
})

const existingCanonicalModelNames = computed(() => {
  return Array.from(new Set(models.value.map((m) => m.name))).sort()
})

// Quick Classify Modal state
const showClassifyModal = ref(false)
const classifySourceModel = ref('')
const classifyTargetModel = ref('')
const classifyCustomTarget = ref('')

function openClassifyDialog(sourceModelName: string, defaultTarget = '') {
  classifySourceModel.value = sourceModelName
  classifyTargetModel.value = defaultTarget || (existingCanonicalModelNames.value[0] || '')
  classifyCustomTarget.value = ''
  showClassifyModal.value = true
}

async function submitClassify() {
  const source = classifySourceModel.value.trim()
  const target = (classifyTargetModel.value === '__custom__'
    ? classifyCustomTarget.value
    : classifyTargetModel.value
  ).trim()
  if (!source || !target) {
    showToast('请选择或输入要归入的目标模型')
    return
  }
  const res = await setCustomModelMapping(source, target)
  if (res) {
    customModelMappings.value = res
  } else {
    await loadCustomModelMappings()
  }
  showClassifyModal.value = false
  showToast(`已成功将“${source}”归入“${target}”`)
}

// Custom Mappings Manager Modal state
const showCustomMappingsModal = ref(false)
const managerSourceModel = ref('')
const managerTargetModel = ref('')
const managerCustomTarget = ref('')

function openCustomMappingsModal() {
  managerSourceModel.value = ''
  managerTargetModel.value = existingCanonicalModelNames.value[0] || ''
  managerCustomTarget.value = ''
  showCustomMappingsModal.value = true
}

async function handleAddManagerMapping() {
  const source = managerSourceModel.value.trim()
  const target = (managerTargetModel.value === '__custom__'
    ? managerCustomTarget.value
    : managerTargetModel.value
  ).trim()
  if (!source || !target) {
    showToast('请填写原始模型与目标模型')
    return
  }
  const res = await setCustomModelMapping(source, target)
  if (res) {
    customModelMappings.value = res
  } else {
    await loadCustomModelMappings()
  }
  managerSourceModel.value = ''
  managerCustomTarget.value = ''
  showToast(`已添加模型归类映射：${source} ➔ ${target}`)
}

async function handleRemoveCustomMapping(raw: string) {
  const res = await deleteCustomModelMapping(raw)
  if (res) {
    customModelMappings.value = res
  } else {
    await loadCustomModelMappings()
  }
  showToast(`已解除“${raw}”的模型归类映射`)
}

// Settings Page form state
const newMappingSource = ref('')
const newMappingTarget = ref('')

async function handleAddSettingsMapping() {
  const source = newMappingSource.value.trim()
  const target = newMappingTarget.value.trim()
  if (!source || !target) return
  const res = await setCustomModelMapping(source, target)
  if (res) {
    customModelMappings.value = res
  } else {
    await loadCustomModelMappings()
  }
  newMappingSource.value = ''
  newMappingTarget.value = ''
  showToast(`已添加模型归类映射：${source} ➔ ${target}`)
}

function copyAlias(text: string) {
  navigator.clipboard.writeText(text).then(() => {
    copiedAliasText.value = text
    showToast(`已复制: ${text}`)
    setTimeout(() => {
      if (copiedAliasText.value === text) {
        copiedAliasText.value = ''
      }
    }, 1800)
  })
}

function copyAllAliases(model: AggregateModelCard) {
  const text = (model.aliases || []).join('\n')
  if (!text) return
  navigator.clipboard.writeText(text).then(() => {
    copiedAliasText.value = '__ALL__'
    showToast(`已复制全部 ${model.aliases.length} 个上游别名`)
    setTimeout(() => {
      if (copiedAliasText.value === '__ALL__') {
        copiedAliasText.value = ''
      }
    }, 1800)
  })
}

const filteredModalAliases = computed(() => {
  const m = activeAliasModalModel.value
  if (!m || !m.aliases) return []
  const q = aliasModalSearch.value.trim().toLowerCase()

  const items = m.aliases.map((alias) => {
    const matchedChannels = enabledChannels.value.filter((ch) => ch.modelNames?.includes(alias))
    return {
      alias,
      channels: matchedChannels.length > 0 ? matchedChannels : m.channelDetails.filter((c) => c.upstreamModel === alias),
    }
  })

  if (!q) return items
  return items.filter((item) => {
    return (
      item.alias.toLowerCase().includes(q) ||
      item.channels.some((c) => c.name.toLowerCase().includes(q))
    )
  })
})

const filteredModalChannelDetails = computed(() => {
  const m = activeAliasModalModel.value
  if (!m || !m.channelDetails) return []
  const q = aliasModalSearch.value.trim().toLowerCase()
  if (!q) return m.channelDetails
  return m.channelDetails.filter((ch) => {
    return (
      ch.name.toLowerCase().includes(q) ||
      ch.upstreamModel.toLowerCase().includes(q)
    )
  })
})

function getModelFamily(name: string): { key: string; label: string; color: string; bg: string; border: string } {
  const n = (name || '').toLowerCase()
  if (n.includes('deepseek')) {
    return { key: 'deepseek', label: 'DeepSeek', color: '#2563eb', bg: 'rgba(37, 99, 235, 0.08)', border: 'rgba(37, 99, 235, 0.2)' }
  }
  if (n.includes('claude')) {
    return { key: 'claude', label: 'Claude', color: '#d97706', bg: 'rgba(217, 119, 6, 0.08)', border: 'rgba(217, 119, 6, 0.2)' }
  }
  if (n.includes('gpt') || n.startsWith('o1') || n.startsWith('o3') || n.includes('chatgpt')) {
    return { key: 'openai', label: 'OpenAI', color: '#059669', bg: 'rgba(5, 150, 105, 0.08)', border: 'rgba(5, 150, 105, 0.2)' }
  }
  if (n.includes('qwen')) {
    return { key: 'qwen', label: '通义千问', color: '#7c3aed', bg: 'rgba(124, 58, 237, 0.08)', border: 'rgba(124, 58, 237, 0.2)' }
  }
  if (n.includes('glm')) {
    return { key: 'glm', label: '智谱 GLM', color: '#4f46e5', bg: 'rgba(79, 70, 229, 0.08)', border: 'rgba(79, 70, 229, 0.2)' }
  }
  if (n.includes('kimi') || n.includes('moonshot')) {
    return { key: 'kimi', label: 'Kimi 月暗', color: '#0284c7', bg: 'rgba(2, 132, 199, 0.08)', border: 'rgba(2, 132, 199, 0.2)' }
  }
  if (n.includes('minimax')) {
    return { key: 'minimax', label: 'MiniMax', color: '#e11d48', bg: 'rgba(225, 29, 72, 0.08)', border: 'rgba(225, 29, 72, 0.2)' }
  }
  if (n.includes('doubao') || n.includes('skylark')) {
    return { key: 'doubao', label: '字节豆包', color: '#0284c7', bg: 'rgba(2, 132, 199, 0.08)', border: 'rgba(2, 132, 199, 0.2)' }
  }
  if (n.includes('gemini')) {
    return { key: 'gemini', label: 'Gemini', color: '#ea580c', bg: 'rgba(234, 88, 12, 0.08)', border: 'rgba(234, 88, 12, 0.2)' }
  }
  if (n.includes('llama')) {
    return { key: 'llama', label: 'Llama', color: '#0891b2', bg: 'rgba(8, 145, 178, 0.08)', border: 'rgba(8, 145, 178, 0.2)' }
  }
  return { key: 'other', label: '通用模型', color: '#64748b', bg: 'rgba(100, 116, 139, 0.08)', border: 'rgba(100, 116, 139, 0.2)' }
}

const modelFamilyTabs = computed(() => {
  const counts: Record<string, number> = {}
  models.value.forEach((m) => {
    const fam = getModelFamily(m.name).key
    counts[fam] = (counts[fam] || 0) + 1
  })
  const tabs = [{ key: 'all', label: '全部', count: models.value.length }]
  const order = ['deepseek', 'claude', 'openai', 'qwen', 'glm', 'kimi', 'minimax', 'doubao', 'gemini', 'llama', 'other']
  const labelMap: Record<string, string> = {
    deepseek: 'DeepSeek',
    claude: 'Claude',
    openai: 'OpenAI',
    qwen: '通义千问',
    glm: '智谱 GLM',
    kimi: 'Kimi',
    minimax: 'MiniMax',
    doubao: '豆包',
    gemini: 'Gemini',
    llama: 'Llama',
    other: '其他',
  }
  order.forEach((k) => {
    if (counts[k]) {
      tabs.push({ key: k, label: labelMap[k] || k, count: counts[k] })
    }
  })
  return tabs
})

const filteredModels = computed(() => {
  let list = models.value
  if (selectedModelFamily.value !== 'all') {
    list = list.filter((m) => getModelFamily(m.name).key === selectedModelFamily.value)
  }
  const query = channelSearch.value.trim().toLowerCase()
  if (!query) return list
  return list.filter((m) => {
    return (
      m.name.toLowerCase().includes(query) ||
      m.providers.some((p) => p.toLowerCase().includes(query)) ||
      m.aliases.some((a) => a.toLowerCase().includes(query)) ||
      m.channelDetails.some((ch) => ch.upstreamModel.toLowerCase().includes(query))
    )
  })
})

const filteredLogs = computed(() => {
  const query = channelSearch.value.trim().toLowerCase()
  if (!query) return desktopLogs.value
  return desktopLogs.value.filter((log) => {
    return (
      (log.model && log.model.toLowerCase().includes(query)) ||
      (log.channel_name && log.channel_name.toLowerCase().includes(query)) ||
      (log.failover_from && log.failover_from.toLowerCase().includes(query)) ||
      String(log.status_code).includes(query)
    )
  })
})

const availableModels = computed(() => new Set(models.value.flatMap((model) => model.providers)).size)
const healthyChannels = computed(() => enabledChannels.value.filter((channel) => channel.status === 'healthy').length)

const averageLatency = computed(() => {
  const activeLatencies = enabledChannels.value.map((c) => c.latency).filter((l): l is number => typeof l === 'number' && l > 0)
  if (!activeLatencies.length) return null
  const sum = activeLatencies.reduce((acc, v) => acc + v, 0)
  return Math.round(sum / activeLatencies.length)
})


function statusLabel(c: Channel) {
  if (c.status === 'healthy') return '正常'
  if (c.status === 'degraded') return '波动'
  if (c.status === 'unknown') return '待探活'
  if (c.lastError) {
    if (c.lastError.includes('401')) return '未授权 (401)'
    if (c.lastError.includes('404')) return '模型不存在 (404)'
    if (c.lastError.includes('429')) return '超额/限流 (429)'
    if (c.lastError.includes('timeout') || c.lastError.includes('deadline')) return '连接超时'
    return '异常'
  }
  return '离线'
}

function getStatusTooltip(c: Channel): string {
  if (c.status === 'healthy') return `渠道与主模型探活正常 (响应耗时: ${c.latency ?? '—'}ms)`
  if (c.status === 'degraded') return `模型正常返回，但时延较高 (${c.latency}ms)`
  if (c.status === 'unknown') return '渠道新建，尚未发起过探活检测'
  if (c.lastError) return `探活失败具体报错：\n${c.lastError}`
  return '渠道离线或网络无法连接'
}

function circuitLabel(state?: Channel['circuitState']) {
  return state === 'open' ? '熔断中' : state === 'half_open' ? '恢复探测' : ''
}

function toggleChannel(channel: Channel) {
  channel.enabled = !channel.enabled
  void persistChannel(channel)
  showToast(`已${channel.enabled ? '启用' : '暂停'}渠道“${channel.name}”`)
}

function fromDesktopChannel(item: DesktopApiChannel): Channel {
  const modelNames = [item.primaryModel, ...item.extraModels].filter(Boolean)
  return {
    name: item.name,
    url: item.url,
    protocol: item.provider,
    status: item.status,
    latency: item.latency,
    models: modelNames.length,
    priority: item.priority,
    lastCheck: item.lastCheck,
    accent: item.provider === 'anthropic' ? '#f59e0b' : item.provider === 'openai-responses' ? '#3b82f6' : '#10b981',
    enabled: item.enabled,
    probeInterval: item.intervalSeconds,
    apiKey: item.apiKey,
    primaryModel: item.primaryModel,
    modelNames,
    probing: false,
    desktopId: item.id,
    failureCount: item.failureCount,
    circuitState: item.circuitState,
    cooldownUntil: item.cooldownUntil,
    lastError: item.lastError,
  }
}

function toDesktopChannel(channel: Channel): DesktopApiChannel {
  const primary = channel.primaryModel || channel.modelNames?.[0] || 'gpt-4o-mini'
  const extras = (channel.modelNames || []).filter((m) => m !== primary)
  return {
    id: channel.desktopId ?? 0,
    name: channel.name,
    provider: channel.protocol,
    apiMode: channel.protocol === 'openai-responses' ? 'responses' : 'chat_completions',
    url: channel.url,
    apiKey: channel.apiKey,
    primaryModel: primary,
    extraModels: extras,
    enabled: channel.enabled,
    intervalSeconds: channel.probeInterval,
    priority: channel.priority,
    status: channel.status,
    latency: channel.latency,
    pingLatency: null,
    lastError: '',
    lastCheck: channel.lastCheck,
    failureCount: channel.failureCount ?? 0,
    circuitState: channel.circuitState ?? 'closed',
    cooldownUntil: channel.cooldownUntil ?? null,
  }
}

async function refreshDesktopChannels() {
  const remote = await listDesktopChannels()
  if (remote === null) return false
  desktopApiActive.value = true
  channels.value = remote.map(fromDesktopChannel)
  return true
}

async function persistChannel(channel: Channel) {
  if (!desktopApiActive.value) {
    await saveChannels()
    return
  }
  const saved = await updateDesktopChannel(toDesktopChannel(channel))
  if (!saved) desktopApiActive.value = false
}

async function probeChannel(channel: Channel) {
  if (channel.probing) return
  channel.probing = true
  if (desktopApiActive.value && channel.desktopId) {
    const details = await probeDesktopChannelDetails(channel.desktopId)
    if (details && details.length > 0) {
      for (const res of details) {
        const key = getModelKey(channel, res.model)
        const isOk = res.status === 'operational' || res.status === 'degraded' || res.status === 'healthy'
        const hasErr = !isOk
        const errMsg = hasErr ? (res.message || '模型调用异常') : null
        modelLatencies.value[key] = {
          latency: res.latencyMs ?? null,
          probing: false,
          status: res.status,
          error: errMsg,
          message: hasErr ? errMsg! : (res.status === 'degraded' ? '响应稍慢' : ''),
        }
      }
    }
    await refreshDesktopChannels()
    channel.probing = false
    const count = details?.length ?? (channel.modelNames?.length || 1)
    const errs = details?.filter(d => d.status !== 'healthy' && d.status !== 'operational').length ?? 0
    if (errs === 0) {
      showToast(`渠道“${channel.name}”共 ${count} 个模型全部探活成功`)
    } else {
      showToast(`渠道“${channel.name}”探活完成，${count - errs}/${count} 个模型正常，${errs} 个异常`)
    }
    return
  }
  await new Promise((r) => setTimeout(r, 600))
  channel.probing = false
  showToast(`渠道“${channel.name}”测试完成`)
}

async function probeAllChannels() {
  await Promise.all(enabledChannels.value.map((channel) => probeChannel(channel)))
  showToast('全量渠道探活完成')
}

async function duplicateChannel(channel: Channel) {
  if (desktopApiActive.value && channel.desktopId) {
    const duplicated = await duplicateDesktopChannel(channel.desktopId)
    if (duplicated) {
      await refreshDesktopChannels()
      showToast(`已成功复制渠道“${channel.name}”`)
      return
    }
  }
  const copy: Channel = {
    ...channel,
    name: `${channel.name} (副本)`,
    desktopId: undefined,
    status: 'offline',
    latency: null,
    lastCheck: '尚未探活',
    priority: channels.value.length + 1,
  }
  channels.value.push(copy)
  await saveChannels()
  showToast(`已成功复制渠道“${channel.name}”`)
}

async function openEditChannel(channel: Channel) {
  editingChannel.value = channel
  editChannelName.value = channel.name
  editChannelUrl.value = channel.url

  let rawKey = channel.apiKey || ''
  if ((!rawKey || rawKey.includes('***')) && channel.desktopId) {
    const fresh = await getDesktopChannel(channel.desktopId, true)
    if (fresh?.apiKey && !fresh.apiKey.includes('***')) {
      rawKey = fresh.apiKey
      channel.apiKey = fresh.apiKey
    }
  }
  editChannelKey.value = rawKey
  editChannelProtocol.value = channel.protocol
  editChannelEnabled.value = channel.enabled
  editChannelPriority.value = channel.priority ?? 1
  showEditKey.value = true
  const interval = channel.probeInterval ?? 0
  if (interval === 0 || interval === 300 || interval === 900 || interval === 1800) {
    editChannelProbePreset.value = interval
  } else {
    editChannelProbePreset.value = -1
    editChannelProbeCustom.value = interval
  }
  editChannelModelText.value = (channel.modelNames?.length ? channel.modelNames : ['gpt-4o-mini']).join('\n')
  showEditChannel.value = true
}

async function saveEditChannel() {
  if (!editingChannel.value) return
  const name = editChannelName.value.trim()
  const rawUrl = editChannelUrl.value.trim()
  const modelNames = modelsFromText(editChannelModelText.value)
  const model = modelNames[0] ?? ''
  const interval = editChannelProbePreset.value === -1 ? Number(editChannelProbeCustom.value) : editChannelProbePreset.value
  if (!name || !rawUrl || !model) {
    showToast('请完整填写渠道名称、Base URL 与探活主模型')
    return
  }
  if (editChannelProbePreset.value === -1 && (interval < 15 || isNaN(interval))) {
    showToast('自定义探活间隔必须大于等于 15 秒')
    return
  }
  const finalUrl = normalizeEndpointUrl(rawUrl)
  const target = editingChannel.value
  target.name = name
  target.url = finalUrl
  target.protocol = editChannelProtocol.value
  target.enabled = editChannelEnabled.value
  target.probeInterval = interval
  target.priority = Math.max(1, Math.floor(Number(editChannelPriority.value) || 1))
  target.modelNames = modelNames
  target.models = target.modelNames.length
  target.accent = target.protocol === 'anthropic' ? '#f59e0b' : target.protocol === 'openai-responses' ? '#3b82f6' : '#10b981'

  if (editChannelKey.value.trim() && !editChannelKey.value.includes('***')) {
    target.apiKey = editChannelKey.value.trim()
  } else if (!editChannelKey.value.trim()) {
    target.apiKey = ''
  }

  await persistChannel(target)
  showEditChannel.value = false
  editingChannel.value = null
  showToast(`渠道“${name}”配置已保存`)
}

async function handleConfirmModalAction() {
  const modal = confirmModal.value
  if (!modal) return
  confirmModal.value = null
  await modal.onConfirm()
}

function removeChannel(channel: Channel) {
  confirmModal.value = {
    title: '删除渠道确认',
    message: `确定要删除供应商渠道 “${channel.name}” 吗？`,
    description: '此操作将移除该渠道及其所有上游路由映射，该操作不可撤销。',
    danger: true,
    confirmText: '确认删除',
    onConfirm: async () => {
      const name = channel.name
      if (desktopApiActive.value && channel.desktopId) {
        const deleted = await deleteDesktopChannel(channel.desktopId)
        if (!deleted) {
          showToast(`删除渠道“${name}”失败，请重试`)
          return
        }
      }
      channels.value = channels.value.filter((item) => item !== channel)
      if (!desktopApiActive.value) {
        await saveChannels()
      }
      showToast(`已成功删除渠道“${name}”`)
    },
  }
}

function saveChannels() {
  return writeDesktopValue('channels', channels.value)
}

async function exportConfig() {
  let exportChannels = channels.value
  if (desktopApiActive.value) {
    const remote = await listDesktopChannels(true)
    if (remote) {
      exportChannels = remote.map(fromDesktopChannel)
    }
  }
  const payload = {
    format: 'heigate-desktop-config',
    version: 1,
    exportedAt: new Date().toISOString(),
    channels: exportChannels,
  }
  const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `heigate-config-${new Date().toISOString().slice(0, 10)}.json`
  anchor.click()
  URL.revokeObjectURL(url)
  showToast('配置已导出为 JSON 文件')
}

function openImport() {
  configInput.value?.click()
}

function clearWorkspace() {
  confirmModal.value = {
    title: '清空工作区确认',
    message: '确定清空本机保存的所有渠道配置吗？',
    description: '此操作不可撤销，所有已保存的渠道及路由映射都将被清除。',
    danger: true,
    confirmText: '确认清空',
    onConfirm: async () => {
      if (desktopApiActive.value) {
        const remote = await listDesktopChannels()
        if (remote) {
          for (const channel of remote) {
            if (!(await deleteDesktopChannel(channel.id))) {
              showToast('清空失败，已有部分渠道未删除')
              return
            }
          }
        }
      }
      channels.value = []
      await clearDesktopValue('channels')
      showToast('本地工作区已清空')
    },
  }
}

// Model Latency & Speed Test helpers
function openModelLatencyModal(channel: Channel) {
  activeModelLatencyChannel.value = channel
  modelSearchQuery.value = ''
  if (channel.latency != null && channel.modelNames?.[0]) {
    const key = `${channel.desktopId ?? channel.name}::${channel.modelNames[0]}`
    if (!modelLatencies.value[key]) {
      const isFailed = channel.status === 'offline' && Boolean(channel.lastError)
      modelLatencies.value[key] = {
        latency: channel.latency,
        status: channel.status || 'healthy',
        error: isFailed ? channel.lastError : null,
      }
    }
  }
}

function isPrimaryModel(channel?: Channel | null, modelName?: string): boolean {
  if (!channel || !modelName) return false
  if (channel.primaryModel) return channel.primaryModel === modelName
  if (channel.modelNames && channel.modelNames.length > 0) return channel.modelNames[0] === modelName
  return false
}

function getModelKey(channel: Channel, modelName: string) {
  return `${channel.desktopId ?? channel.name}::${modelName}`
}

function isModelFailed(channel: Channel, modelName: string): boolean {
  if (channel.probing) return false
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry) {
    if (entry.probing) return false
    return Boolean(entry.error || entry.status === 'error' || entry.status === 'failed' || entry.status === 'timeout')
  }
  if (channel.lastError && (channel.lastError.includes(`[${modelName}]`) || channel.lastError.includes(modelName))) {
    return true
  }
  return false
}

function getModelErrorMessage(channel: Channel, modelName: string): string {
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry?.error) return entry.error
  if (entry?.message) return entry.message
  if (channel.lastError && (channel.lastError.includes(`[${modelName}]`) || channel.lastError.includes(modelName))) {
    return channel.lastError
  }
  return channel.lastError || '模型调用异常'
}

function getModelLatencyDisplay(channel: Channel, modelName: string): string {
  if (isModelProbing(channel, modelName)) return '测速中...'
  if (isModelFailed(channel, modelName)) {
    const err = getModelErrorMessage(channel, modelName)
    const codeMatch = err.match(/HTTP(?:\/[0-9.]+)?\s*([1-5]\d{2})\b/i) || err.match(/\b([45]\d{2})\b/)
    if (codeMatch) return `HTTP ${codeMatch[1]}`
    if (err.includes('timeout') || err.includes('超时') || err.includes('slow response')) return '超时'
    if (err.includes('mismatch')) return '校验失败'
    if (err.includes('refused') || err.includes('reset')) return '网络中断'
    return '异常'
  }
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry?.latency != null) return `${entry.latency}ms`
  if (channel.latency != null && channel.status !== 'offline') {
    return `${channel.latency}ms`
  }
  return '待测速'
}

function getModelLatencyClass(channel: Channel, modelName: string): string {
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry?.probing || channel.probing) return 'probing'
  if (isModelFailed(channel, modelName)) {
    return 'timeout'
  }
  const lat = entry?.latency ?? (channel.latency != null && channel.status !== 'offline' ? channel.latency : null)
  if (lat == null) return 'idle'
  if (lat < 1000) return 'fast'
  if (lat < 2500) return 'normal'
  return 'slow'
}

function getModelTooltip(channel: Channel, modelName: string): string {
  if (channel.probing) return '正在进行通道探活检测...'
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry?.probing) return '正在向该模型发起真实测试 Prompt 进行端到端检验...'
  if (isModelFailed(channel, modelName)) {
    return `测速异常：${getModelErrorMessage(channel, modelName)} (点击查看详情与诊断)`
  }
  if (entry?.latency != null) {
    return `模型工作正常，真实对话端到端响应耗时 ${entry.latency}ms`
  }
  return '该模型响应就绪，点击可在弹窗中进行单个真实测速'
}

function isModelProbing(channel: Channel, modelName: string): boolean {
  if (channel.probing) return true
  return Boolean(modelLatencies.value[getModelKey(channel, modelName)]?.probing)
}

function getModelSortScore(channel: Channel, modelName: string): { rank: number; latency: number; name: string } {
  const isFailed = isModelFailed(channel, modelName)
  const isProbing = isModelProbing(channel, modelName)
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]

  // 1. 失败 / 报错 / 超时 -> 最底端 (Rank 5)
  if (isFailed) {
    return { rank: 5, latency: 999999, name: modelName }
  }

  // 2. 测速中 -> 倒数第二 (Rank 4)
  if (isProbing) {
    return { rank: 4, latency: 999999, name: modelName }
  }

  // 3. 已测出有效延迟的模型 (Rank 1 正常 / Rank 2 稍慢，按实际延迟从小到大排序)
  if (entry?.latency != null && entry.latency > 0) {
    if (entry.status === 'degraded' || entry.latency >= 3000) {
      return { rank: 2, latency: entry.latency, name: modelName }
    }
    return { rank: 1, latency: entry.latency, name: modelName }
  }

  // 4. 探活主模型默认继承的渠道有效延迟 (若渠道本身 healthy)
  const isPrimary = isPrimaryModel(channel, modelName)
  if (isPrimary && channel.latency != null && channel.latency > 0 && channel.status !== 'offline') {
    if (channel.status === 'degraded' || channel.latency >= 3000) {
      return { rank: 2, latency: channel.latency, name: modelName }
    }
    return { rank: 1, latency: channel.latency, name: modelName }
  }

  // 5. 待测速 / 未测状态 (Rank 3)
  return { rank: 3, latency: 999999, name: modelName }
}

const filteredModalModels = computed(() => {
  const ch = activeModelLatencyChannel.value
  if (!ch) return []
  const list = ch.modelNames?.length ? [...ch.modelNames] : [ch.name]
  const q = modelSearchQuery.value.trim().toLowerCase()
  const filtered = q ? list.filter((m) => m.toLowerCase().includes(q)) : list

  return filtered.sort((a, b) => {
    const sA = getModelSortScore(ch, a)
    const sB = getModelSortScore(ch, b)
    if (sA.rank !== sB.rank) return sA.rank - sB.rank
    if (sA.latency !== sB.latency) return sA.latency - sB.latency
    return sA.name.localeCompare(sB.name)
  })
})

async function probeSingleModel(channel: Channel, modelName: string) {
  const key = getModelKey(channel, modelName)
  modelLatencies.value[key] = { ...(modelLatencies.value[key] || {}), probing: true }
  const t0 = performance.now()
  try {
    if (desktopApiActive.value && channel.desktopId) {
      const probeRes = await probeDesktopChannelDetails(channel.desktopId, modelName)
      const res = probeRes?.[0]
      if (res) {
        const isOk = res.status === 'operational' || res.status === 'degraded'
        const lat = res.latencyMs ?? (isOk ? Math.round(performance.now() - t0) : null)
        const hasErr = !isOk
        const errMsg = hasErr ? (res.message || '模型调用异常') : null
        modelLatencies.value[key] = {
          latency: lat,
          probing: false,
          status: res.status,
          error: errMsg,
          message: hasErr ? errMsg! : (res.status === 'degraded' ? '响应稍慢' : ''),
        }
        if (channel.modelNames?.[0] === modelName) {
          channel.latency = lat
          if (isOk) {
            channel.status = res.status === 'degraded' ? 'degraded' : 'healthy'
            channel.lastError = ''
          } else {
            channel.status = 'offline'
            channel.lastError = errMsg!
          }
        }
        return
      }
    }
    const testUrl = `${normalizeEndpointUrl(channel.url)}/models`
    const resp = await fetch(testUrl, {
      headers: {
        Accept: 'application/json',
        ...(channel.apiKey && !channel.apiKey.includes('***') ? { Authorization: `Bearer ${channel.apiKey}`, 'x-api-key': channel.apiKey } : {}),
      },
      signal: AbortSignal.timeout(5000),
    })
    const elapsed = Math.round(performance.now() - t0)
    const isOk = resp.ok
    const errMsg = isOk ? null : `上游 HTTP ${resp.status}`
    modelLatencies.value[key] = {
      latency: isOk ? elapsed : null,
      probing: false,
      status: isOk ? 'operational' : 'error',
      error: errMsg,
      message: isOk ? '' : errMsg!,
    }
  } catch (err: any) {
    const errMsg = err?.message || '请求超时或网络异常'
    modelLatencies.value[key] = {
      latency: null,
      probing: false,
      status: 'timeout',
      error: errMsg,
      message: errMsg,
    }
  }
}

async function probeAllChannelModels(channel: Channel) {
  if (isProbingAllModels.value) return
  isProbingAllModels.value = true
  const models = channel.modelNames ?? []

  for (const m of models) {
    const key = getModelKey(channel, m)
    modelLatencies.value[key] = { ...(modelLatencies.value[key] || {}), probing: true }
  }

  try {
    if (desktopApiActive.value && channel.desktopId) {
      const probeRes = await probeDesktopChannelDetails(channel.desktopId)
      if (probeRes && probeRes.length > 0) {
        for (const res of probeRes) {
          const key = getModelKey(channel, res.model)
          const isOk = res.status === 'operational' || res.status === 'degraded'
          const hasErr = !isOk
          const errMsg = hasErr ? (res.message || '模型调用异常') : null
          modelLatencies.value[key] = {
            latency: res.latencyMs ?? null,
            probing: false,
            status: res.status,
            error: errMsg,
            message: hasErr ? errMsg! : (res.status === 'degraded' ? '响应稍慢' : ''),
          }
        }
        await refreshDesktopChannels()
        isProbingAllModels.value = false
        showToast(`已完成渠道“${channel.name}”全量真实模型调用测速`)
        return
      }
    }
  } catch {
    // Fall back to probing models one by one below.
  }

  for (const m of models) {
    await probeSingleModel(channel, m)
  }
  isProbingAllModels.value = false
  showToast(`已完成渠道“${channel.name}”全量 ${models.length} 个模型的测速`)
}

function copyModelName(name: string) {
  navigator.clipboard.writeText(name).then(() => {
    showToast(`已复制模型 ID: ${name}`)
  }).catch(() => {
    showToast(`模型 ID: ${name}`)
  })
}

function getFailedModelsInChannel(channel?: Channel | null): string[] {
  if (!channel || !channel.modelNames || channel.modelNames.length <= 1) return []
  return channel.modelNames.filter((m) => isModelFailed(channel, m))
}

async function removeModelFromChannel(channel?: Channel | null, modelName?: string) {
  if (!channel || !modelName) return
  if (!channel.modelNames || channel.modelNames.length <= 1) {
    showToast('渠道至少需要保留一个模型，无法继续删除')
    return
  }

  const isPrimary = isPrimaryModel(channel, modelName)
  confirmModal.value = {
    title: '删除模型确认',
    message: `确定要从渠道 “${channel.name}” 中移除模型 “${modelName}” 吗？`,
    description: isPrimary
      ? '注意：该模型是当前渠道的探活主模型，删除后将自动将下一顺位模型提升为主模型。'
      : '删除后，网关将不再通过该渠道分发该模型的请求。',
    danger: true,
    confirmText: '确认删除',
    onConfirm: async () => {
      const remaining = (channel.modelNames || []).filter((m) => m !== modelName)
      channel.modelNames = remaining
      channel.models = remaining.length
      if (isPrimary || channel.primaryModel === modelName) {
        channel.primaryModel = remaining[0]
      }
      const key = getModelKey(channel, modelName)
      delete modelLatencies.value[key]

      await persistChannel(channel)
      await refreshDesktopChannels()
      showToast(`已从渠道“${channel.name}”删除模型“${modelName}”`)
    },
  }
}

async function cleanFailedModels(channel?: Channel | null) {
  if (!channel) return
  const failedList = getFailedModelsInChannel(channel)
  if (failedList.length === 0) {
    showToast('当前渠道未检测到失效或报错的模型')
    return
  }
  const remainingCount = (channel.modelNames?.length || 0) - failedList.length
  if (remainingCount < 1) {
    showToast('不可全部清除，渠道至少需要保留一个模型')
    return
  }

  confirmModal.value = {
    title: '批量清理失效模型',
    message: `确定要一键清理渠道 “${channel.name}” 中全部 ${failedList.length} 个报错/失效模型吗？`,
    description: `清理后将保留其余 ${remainingCount} 个可用模型。`,
    danger: true,
    confirmText: `清理 ${failedList.length} 个模型`,
    onConfirm: async () => {
      const remaining = (channel.modelNames || []).filter((m) => !failedList.includes(m))
      channel.modelNames = remaining
      channel.models = remaining.length
      if (failedList.includes(channel.primaryModel || '')) {
        channel.primaryModel = remaining[0]
      }
      failedList.forEach((m) => {
        delete modelLatencies.value[getModelKey(channel, m)]
      })

      await persistChannel(channel)
      await refreshDesktopChannels()
      showToast(`已成功从渠道“${channel.name}”清理 ${failedList.length} 个失效模型`)
    },
  }
}

// ============================================================================
// Channel Probe Diagnostic & Error Inspection
// ============================================================================
const activeDiagnosticChannel = ref<Channel | null>(null)

function openChannelDiagnostic(c: Channel) {
  activeDiagnosticChannel.value = c
}

function formatErrorSummary(rawError?: string): string {
  if (!rawError) return '未知异常'
  if (rawError.includes('404')) {
    if (rawError.includes('model route not found') || rawError.includes('NOT_FOUND')) {
      return 'HTTP 404: 上游模型路由不存在 (NOT_FOUND)'
    }
    return 'HTTP 404: 目标端点或模型不存在 (Not Found)'
  }
  if (rawError.includes('401')) {
    return 'HTTP 401: API Key 鉴权失败，客户端未授权 (UNAUTHENTICATED)'
  }
  if (rawError.includes('429')) {
    return 'HTTP 429: 上游超频限流或账户额度耗尽 (Rate limit / Quota exceeded)'
  }
  if (rawError.includes('timeout') || rawError.includes('deadline')) {
    return '网络请求超时: 上游未在规定时间内返回响应'
  }
  if (rawError.includes('connection refused')) {
    return '连接被拒绝: 目标服务器端口未开放或防火墙阻拦'
  }
  return rawError.length > 90 ? rawError.slice(0, 90) + '...' : rawError
}

function extractErrorCode(rawError?: string): string {
  if (!rawError) return 'ERROR'
  if (rawError.includes('404')) return 'HTTP 404'
  if (rawError.includes('401')) return 'HTTP 401'
  if (rawError.includes('429')) return 'HTTP 429'
  if (rawError.includes('500')) return 'HTTP 500'
  if (rawError.includes('502')) return 'HTTP 502'
  if (rawError.includes('504')) return 'HTTP 504'
  if (rawError.includes('timeout')) return 'TIMEOUT'
  return 'FAIL'
}

function getTroubleshootingAdvice(c: Channel): string[] {
  const err = c.lastError || ''
  const tips: string[] = []
  if (err.includes('404')) {
    tips.push(`<strong>核对探活主模型</strong>：上游可能未开通当前主模型（<code>${c.modelNames?.[0] || '未知'}</code>），或者官方已迭代该版本。若使用商汤建议切换为 <code>sensenova-6.8-flash-lite</code> 或 <code>deepseek-v4-flash</code>。`)
    tips.push(`<strong>检查 Base URL 格式</strong>：当前为 <code>${c.url}</code>。请确认该地址是否需要或多加了版本路径（如 <code>/v1</code>）。`)
  } else if (err.includes('401')) {
    tips.push(`<strong>检查 API 密钥 (Key)</strong>：上游返回未授权。请点击下方「修改渠道配置」，确认填写的 API Key 是否正确、前后有无多余空格，或上游账号该 Key 是否已被删除。`)
    tips.push(`<strong>检查 Key 权限与分组</strong>：部分上游服务（如 OneAPI / NewAPI / 中转站）要求令牌必须绑定特定模型或分组才能调用。`)
  } else if (err.includes('429')) {
    tips.push(`<strong>检查账号额度</strong>：上游返回并发或配额超限，请登录服务商控制台查看账户余额是否已用尽。`)
    tips.push(`<strong>调大探活周期</strong>：在渠道设置中将探活频率从高频调整为「仅手动探活」或较长周期，避免探活本身占用频控配额。`)
  } else if (err.includes('timeout') || err.includes('deadline')) {
    tips.push(`<strong>检查本地网络与代理</strong>：当前计算机到目标服务 <code>${c.url}</code> 连接超时，请确认网络代理或梯子配置。`)
  } else {
    tips.push(`<strong>查看原始报错信息</strong>：参考下方上游完整返回体，排查上游服务给出的具体错误原因。`)
  }
  tips.push(`<strong>一键修改并重试</strong>：您可以直接点击下方「修改渠道配置」更新模型或 Key，保存后点击「重新检测」验证。`)
  return tips
}

function copyDiagnosticRaw() {
  if (!activeDiagnosticChannel.value?.lastError) return
  navigator.clipboard.writeText(activeDiagnosticChannel.value.lastError).then(() => {
    showToast('已复制完整报错数据到剪贴板')
  })
}

function openEditChannelFromDiag(c: Channel) {
  activeDiagnosticChannel.value = null
  openEditChannel(c)
}

async function retryProbeFromDiag(c: Channel) {
  await probeChannel(c)
  const updated = channels.value.find((ch) => ch.desktopId === c.desktopId || ch.name === c.name)
  if (updated) {
    if (updated.status === 'healthy' || !updated.lastError) {
      activeDiagnosticChannel.value = null
      showToast(`渠道“${c.name}”探活测试通过，状态已恢复正常！`)
    } else {
      activeDiagnosticChannel.value = updated
    }
  }
}

function openModelErrorDetail(channel: Channel, modelName: string) {
  if (isModelFailed(channel, modelName)) {
    const errMsg = getModelErrorMessage(channel, modelName)
    const key = getModelKey(channel, modelName)
    const entry = modelLatencies.value[key]
    activeDiagnosticChannel.value = {
      ...channel,
      modelNames: [modelName, ...(channel.modelNames?.filter((m) => m !== modelName) || [])],
      lastError: errMsg,
      latency: entry?.latency ?? channel.latency,
    }
  } else {
    const lat = getModelLatencyDisplay(channel, modelName)
    showToast(`${modelName}：当前响应延迟 ${lat}`)
  }
}

async function importConfig(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    const parsed = JSON.parse(await file.text())
    const isValidFormat = parsed?.format === 'heigate-desktop-config' || parsed?.format === 'sub2api-desktop-config'
    if (!isValidFormat || !Array.isArray(parsed.channels)) throw new Error('invalid config')
    const imported = parsed.channels.filter((channel: Channel) => channel?.name && channel?.url)
    if (desktopApiActive.value) {
      const remote = await listDesktopChannels()
      const created: DesktopApiChannel[] = []
      for (const channel of imported) {
        const saved = await createDesktopChannel({ ...toDesktopChannel(channel), apiKey: channel.apiKey ?? '' })
        if (!saved) {
          await Promise.all(created.map((channel) => deleteDesktopChannel(channel.id)))
          throw new Error('remote import failed')
        }
        created.push(saved)
      }
      if (remote) {
        for (const channel of remote) {
          if (!(await deleteDesktopChannel(channel.id))) {
            throw new Error('remote cleanup failed')
          }
        }
      }
      await refreshDesktopChannels()
    } else {
      channels.value = imported
      await saveChannels()
    }
    activeSection.value = 'channels'
    showToast(`成功导入 ${imported.length} 个渠道`)
  } catch {
    showToast('配置文件格式不正确，请确保为有效的 JSON')
  } finally {
    input.value = ''
  }
}

async function refreshAllData() {
  await Promise.allSettled([
    refreshDesktopChannels(),
    refreshGatewayHealth(),
    refreshSystemInfo(),
    refreshLogs(),
  ])
  showToast('已刷新网关状态与全部数据')
}

const showShortcutsModal = ref(false)
const selectedChannelId = ref<string | number | null>(null)

function isChannelSelected(c: Channel): boolean {
  if (selectedChannelId.value == null) return false
  return (c.desktopId != null && c.desktopId === selectedChannelId.value) || c.name === selectedChannelId.value
}

function selectChannel(c: Channel) {
  selectedChannelId.value = c.desktopId ?? c.name
}

function getSelectedChannel(): Channel | null {
  if (selectedChannelId.value == null) return null
  return sortedChannels.value.find(c => (c.desktopId != null && c.desktopId === selectedChannelId.value) || c.name === selectedChannelId.value) || null
}

function selectNextChannel(delta: number) {
  const list = sortedChannels.value
  if (!list.length) return
  let idx = list.findIndex(c => isChannelSelected(c))
  if (idx === -1) {
    idx = delta > 0 ? 0 : list.length - 1
  } else {
    idx = (idx + delta + list.length) % list.length
  }
  selectedChannelId.value = list[idx].desktopId ?? list[idx].name
}

function hasAnyOpenModal(): boolean {
  return Boolean(
    activeLogDetail.value ||
    confirmModal.value ||
    activeDiagnosticChannel.value ||
    activeModelLatencyChannel.value ||
    activeModelSpeedTestName.value ||
    activeAliasModalModel.value ||
    showClassifyModal.value ||
    showCustomMappingsModal.value ||
    showAddChannel.value ||
    showEditChannel.value ||
    showShortcutsModal.value
  )
}

function closeTopModal(): boolean {
  if (showShortcutsModal.value) {
    showShortcutsModal.value = false
    return true
  }
  if (activeModelSpeedTestName.value) {
    closeModelSpeedTestModal()
    return true
  }
  if (showClassifyModal.value) {
    showClassifyModal.value = false
    return true
  }
  if (showCustomMappingsModal.value) {
    showCustomMappingsModal.value = false
    return true
  }
  if (activeAliasModalModel.value) {
    activeAliasModalModel.value = null
    return true
  }
  if (activeLogDetail.value) {
    activeLogDetail.value = null
    return true
  }
  if (confirmModal.value) {
    confirmModal.value = null
    return true
  }
  if (activeDiagnosticChannel.value) {
    activeDiagnosticChannel.value = null
    return true
  }
  if (activeModelLatencyChannel.value) {
    activeModelLatencyChannel.value = null
    return true
  }
  if (showAddChannel.value) {
    showAddChannel.value = false
    return true
  }
  if (showEditChannel.value) {
    showEditChannel.value = false
    return true
  }
  return false
}

function isTypingInInput(): boolean {
  const el = document.activeElement
  if (!el) return false
  const tag = el.tagName.toLowerCase()
  return tag === 'input' || tag === 'textarea' || tag === 'select' || (el as HTMLElement).isContentEditable
}

function onGlobalKeydown(e: KeyboardEvent) {
  const isCmdOrCtrl = e.metaKey || e.ctrlKey

  // 1. Esc: Close top modal, or blur active input, or clear selection
  if (e.key === 'Escape') {
    if (closeTopModal()) {
      e.preventDefault()
      return
    }
    if (isTypingInInput()) {
      e.preventDefault()
      ;(document.activeElement as HTMLElement)?.blur()
      return
    }
    if (selectedChannelId.value != null) {
      e.preventDefault()
      selectedChannelId.value = null
      return
    }
    return
  }

  // 2. ⌘+W: Close top modal
  if (isCmdOrCtrl && (e.key === 'w' || e.key === 'W') && !e.shiftKey) {
    if (closeTopModal()) {
      e.preventDefault()
      return
    }
  }

  // 3. ⌘+Enter: Submit active modal
  if (isCmdOrCtrl && e.key === 'Enter') {
    if (confirmModal.value) {
      e.preventDefault()
      void handleConfirmModalAction()
      return
    }
    if (showAddChannel.value) {
      e.preventDefault()
      void addChannel()
      return
    }
    if (showEditChannel.value) {
      e.preventDefault()
      void saveEditChannel()
      return
    }
  }

  // 4. ⌘+/ or ?: Toggle keyboard shortcuts cheat sheet
  if ((isCmdOrCtrl && e.key === '/') || (!isTypingInInput() && e.key === '?')) {
    e.preventDefault()
    showShortcutsModal.value = !showShortcutsModal.value
    return
  }

  // 5. ⌘+,: macOS Preferences shortcut -> open Settings
  if (isCmdOrCtrl && e.key === ',') {
    e.preventDefault()
    activeSection.value = 'settings'
    return
  }

  // 6. ⌘+1 ~ ⌘+6: Switch tabs
  if (isCmdOrCtrl && !e.shiftKey && !e.altKey) {
    const tabMap: Record<string, typeof activeSection.value> = {
      '1': 'overview',
      '2': 'analytics',
      '3': 'channels',
      '4': 'models',
      '5': 'logs',
      '6': 'settings',
    }
    if (tabMap[e.key]) {
      e.preventDefault()
      activeSection.value = tabMap[e.key]
      return
    }
  }

  // 7. ⌘+K or ⌘+F: Focus search
  if (isCmdOrCtrl && (e.key === 'k' || e.key === 'K' || e.key === 'f' || e.key === 'F')) {
    e.preventDefault()
    if (activeSection.value !== 'channels' && activeSection.value !== 'models' && activeSection.value !== 'logs') {
      activeSection.value = 'channels'
    }
    nextTick(() => {
      searchInputRef.value?.focus()
      searchInputRef.value?.select()
    })
    return
  }

  // 8. ⌘+N: New channel
  if (isCmdOrCtrl && (e.key === 'n' || e.key === 'N') && !e.shiftKey) {
    e.preventDefault()
    showAddChannel.value = true
    return
  }

  // 9. ⌘+Shift+P or ⌘+P (outside input): Batch probe all channels
  if ((isCmdOrCtrl && e.shiftKey && (e.key === 'p' || e.key === 'P')) || (isCmdOrCtrl && (e.key === 'p' || e.key === 'P') && !isTypingInInput())) {
    e.preventDefault()
    void probeAllChannels()
    return
  }

  // 10. ⌘+R: Refresh data
  if (isCmdOrCtrl && (e.key === 'r' || e.key === 'R') && !e.shiftKey) {
    e.preventDefault()
    void refreshAllData()
    return
  }

  // 11. ⌘+E: Edit selected channel
  if (isCmdOrCtrl && (e.key === 'e' || e.key === 'E') && !e.shiftKey) {
    e.preventDefault()
    const ch = getSelectedChannel() || sortedChannels.value[0]
    if (ch) {
      void openEditChannel(ch)
    }
    return
  }

  // 12. Arrow navigation and channel actions when not inside input and no modal is open
  if (!isTypingInInput() && !hasAnyOpenModal() && activeSection.value === 'channels') {
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
      e.preventDefault()
      selectNextChannel(1)
      return
    }
    if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
      e.preventDefault()
      selectNextChannel(-1)
      return
    }

    const current = getSelectedChannel()
    if (current) {
      if (e.key === 'Enter' || e.key === 'e' || e.key === 'E') {
        e.preventDefault()
        void openEditChannel(current)
        return
      }
      if (e.key === 'p' || e.key === 'P') {
        e.preventDefault()
        void probeChannel(current)
        return
      }
      if (e.key === ' ' || e.code === 'Space') {
        e.preventDefault()
        toggleChannel(current)
        return
      }
      if (!isCmdOrCtrl && (e.key === 'c' || e.key === 'C')) {
        const sel = window.getSelection()?.toString()
        if (!sel) {
          e.preventDefault()
          duplicateChannel(current)
          return
        }
      }
      if (e.key === 'Backspace' || e.key === 'Delete') {
        e.preventDefault()
        removeChannel(current)
        return
      }
    }
  }
}

function onDesktopShortcut(e: Event) {
  const customEvent = e as CustomEvent<string>
  const action = customEvent.detail
  if (action === 'new-channel') {
    showAddChannel.value = true
  } else if (action === 'search') {
    if (activeSection.value !== 'channels' && activeSection.value !== 'models' && activeSection.value !== 'logs') {
      activeSection.value = 'channels'
    }
    nextTick(() => {
      searchInputRef.value?.focus()
      searchInputRef.value?.select()
    })
  } else if (action === 'edit-first-channel') {
    const ch = getSelectedChannel() || sortedChannels.value[0]
    if (ch) {
      void openEditChannel(ch)
    }
  } else if (action === 'refresh') {
    void refreshAllData()
  } else if (action === 'settings') {
    activeSection.value = 'settings'
  } else if (action === 'probe-all') {
    void probeAllChannels()
  } else if (action === 'shortcuts-help') {
    showShortcutsModal.value = true
  }
}

onMounted(async () => {
  document.title = 'HeiGate'
  document.documentElement.classList.remove('dark')
  window.addEventListener('keydown', onGlobalKeydown)
  window.addEventListener('desktop-shortcut', onDesktopShortcut)
  checkTabsFit()
  window.addEventListener('resize', checkTabsFit)
  if (typeof ResizeObserver !== 'undefined' && navCenterRef.value) {
    navCenterObserver = new ResizeObserver(() => checkTabsFit())
    navCenterObserver.observe(navCenterRef.value)
  }
  const saved = await readDesktopValue<Channel[]>('channels')
  if (Array.isArray(saved)) channels.value = saved
  await refreshDesktopChannels()
  await loadCustomModelMappings()
  gatewayApiKey.value = (await getDesktopGatewayKey()) ?? ''
  await refreshGatewayHealth()
  await refreshSystemInfo()
  await refreshLogs()
  await loadDesktopClients()
  startLogsStream()
})

onBeforeUnmount(() => {
  stopLogsStream()
  window.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('desktop-shortcut', onDesktopShortcut)
  window.removeEventListener('resize', checkTabsFit)
  if (navCenterObserver) {
    navCenterObserver.disconnect()
    navCenterObserver = null
  }
})

watch(channels, () => { if (!desktopApiActive.value) void saveChannels() }, { deep: true })

watch(activeSection, (section) => {
  if (section === 'logs') {
    void refreshLogs()
  } else if (section === 'analytics') {
    void loadAnalytics()
  } else if (section === 'overview') {
    void refreshLogs()
    void loadDesktopClients()
  } else if (section === 'settings') {
    void refreshSystemInfo()
    void loadDesktopClients()
  }
})

async function refreshGatewayHealth() {
  const health = await checkLocalGateway()
  gatewayOnline.value = health.ok
  gatewayLatency.value = health.latency
  gatewayLastChecked.value = health.ok ? `${health.latency}ms 响应` : '无法连接'
}

function copyModelId(modelName: string) {
  navigator.clipboard.writeText(modelName).then(() => {
    copiedModelId.value = modelName
    showToast(`已复制模型 ID: ${modelName}`)
    setTimeout(() => {
      if (copiedModelId.value === modelName) copiedModelId.value = ''
    }, 2000)
  })
}

interface ModelProviderSpeedTestItem {
  channel: Channel
  upstreamModel: string
  isPrimary: boolean
  latencyMs: number | null
  latencyDisplay: string
  latencyClass: string
  isProbing: boolean
  isFailed: boolean
  errorMessage: string
  tooltip: string
}

const activeModelProviders = computed<ModelProviderSpeedTestItem[]>(() => {
  const modelName = activeModelSpeedTestName.value
  if (!modelName) return []

  const list: ModelProviderSpeedTestItem[] = []
  channels.value.forEach((ch) => {
    const upstream = ch.modelNames?.find((n) => isModelEquivalentFrontend(n, modelName))
    if (!upstream) return

    const key = getModelKey(ch, upstream)
    const entry = modelLatencies.value[key]
    const isProbing = Boolean(entry?.probing || ch.probing)
    const isFailed = !isProbing && isModelFailed(ch, upstream)
    const errMsg = isFailed ? getModelErrorMessage(ch, upstream) : ''
    const latMs = entry?.latency != null ? entry.latency : (ch.modelNames?.[0] === upstream && ch.latency != null && ch.status !== 'offline' ? ch.latency : null)

    list.push({
      channel: ch,
      upstreamModel: upstream,
      isPrimary: isPrimaryModel(ch, upstream),
      latencyMs: latMs,
      latencyDisplay: getModelLatencyDisplay(ch, upstream),
      latencyClass: getModelLatencyClass(ch, upstream),
      isProbing,
      isFailed,
      errorMessage: errMsg,
      tooltip: getModelTooltip(ch, upstream),
    })
  })

  const q = modelSpeedTestQuery.value.trim().toLowerCase()
  let filtered = list
  if (q) {
    filtered = list.filter((item) =>
      item.channel.name.toLowerCase().includes(q) ||
      item.channel.url.toLowerCase().includes(q) ||
      item.channel.protocol.toLowerCase().includes(q) ||
      item.upstreamModel.toLowerCase().includes(q)
    )
  }

  return filtered.sort((a, b) => {
    if (a.channel.enabled !== b.channel.enabled) {
      return a.channel.enabled ? -1 : 1
    }
    const score = (item: ModelProviderSpeedTestItem) => {
      if (item.isFailed) return 9999999
      if (item.isProbing) return 9999990
      if (item.latencyMs != null && item.latencyMs > 0) return item.latencyMs
      return 9999900
    }
    return score(a) - score(b)
  })
})

function isFastestProvider(item: ModelProviderSpeedTestItem): boolean {
  const healthy = activeModelProviders.value.filter(
    (p) => p.channel.enabled && !p.isFailed && !p.isProbing && p.latencyMs != null && p.latencyMs > 0
  )
  return healthy.length > 0 && healthy[0].channel.name === item.channel.name && healthy[0].upstreamModel === item.upstreamModel
}

function getFastestProviderInfo(modelName?: string | null): { channelName: string; latency: number } | null {
  if (!modelName) return null
  const providers = channels.value.map((ch) => {
    const upstream = ch.modelNames?.find((n) => isModelEquivalentFrontend(n, modelName))
    if (!upstream) return null
    const key = getModelKey(ch, upstream)
    const entry = modelLatencies.value[key]
    const latMs = entry?.latency != null ? entry.latency : (ch.modelNames?.[0] === upstream && ch.latency != null && ch.status !== 'offline' ? ch.latency : null)
    const failed = isModelFailed(ch, upstream)
    if (!ch.enabled || failed || latMs == null || latMs <= 0) return null
    return { channelName: ch.name, latency: latMs }
  }).filter((x): x is { channelName: string; latency: number } => x !== null)

  if (!providers.length) return null
  providers.sort((a, b) => a.latency - b.latency)
  return providers[0]
}

async function probeAllProvidersForModel(modelName?: string | null) {
  const target = modelName || activeModelSpeedTestName.value
  if (!target) return
  if (isProbingModelProviders.value) return
  isProbingModelProviders.value = true

  const matchingChannels: Array<{ channel: Channel; upstreamModel: string }> = []
  channels.value.forEach((ch) => {
    const upstream = ch.modelNames?.find((n) => isModelEquivalentFrontend(n, target))
    if (upstream) {
      matchingChannels.push({ channel: ch, upstreamModel: upstream })
    }
  })

  if (matchingChannels.length === 0) {
    isProbingModelProviders.value = false
    showToast(`暂无提供模型 “${target}” 的供应商渠道`)
    return
  }

  matchingChannels.forEach(({ channel, upstreamModel }) => {
    const key = getModelKey(channel, upstreamModel)
    modelLatencies.value[key] = {
      ...(modelLatencies.value[key] || {}),
      probing: true,
    }
  })

  await Promise.allSettled(
    matchingChannels.map(({ channel, upstreamModel }) =>
      probeSingleModel(channel, upstreamModel)
    )
  )

  isProbingModelProviders.value = false

  const healthyCount = matchingChannels.filter(({ channel, upstreamModel }) => {
    const key = getModelKey(channel, upstreamModel)
    const entry = modelLatencies.value[key]
    return entry?.latency != null && entry.latency > 0 && !isModelFailed(channel, upstreamModel)
  }).length

  const fastest = getFastestProviderInfo(target)
  if (fastest) {
    showToast(`测速完成：${target} 优选供应商为 “${fastest.channelName}” (${fastest.latency}ms)`)
  } else {
    showToast(`已完成 “${target}” 全部 ${matchingChannels.length} 家供应商测速 (可用: ${healthyCount}/${matchingChannels.length})`)
  }
}

function openModelSpeedTestFor(modelName: string) {
  activeModelSpeedTestName.value = modelName
  modelSpeedTestQuery.value = ''
  void probeAllProvidersForModel(modelName)
}

function closeModelSpeedTestModal() {
  activeModelSpeedTestName.value = null
  modelSpeedTestQuery.value = ''
}

async function probeAllActiveModels() {
  if (isProbingAllModels.value) return
  isProbingAllModels.value = true
  showToast('开始全量并发测速所有可用渠道与模型...')
  try {
    await probeAllChannels()
  } finally {
    isProbingAllModels.value = false
  }
}

function openLogDetail(log: DesktopRequestLog) {
  activeLogDetail.value = log
}

function copyLogJson(log: DesktopRequestLog) {
  navigator.clipboard.writeText(JSON.stringify(log, null, 2)).then(() => {
    showToast('已复制完整日志 JSON 到剪贴板')
  })
}

function copyErrorMessage(msg: string) {
  navigator.clipboard.writeText(msg).then(() => {
    showToast('已复制报错信息到剪贴板')
  })
}

function parseTraceSteps(log: DesktopRequestLog): DesktopTraceStep[] {
  if (log.trace_json) {
    try {
      const parsed = JSON.parse(log.trace_json)
      if (Array.isArray(parsed) && parsed.length > 0) {
        return parsed
      }
    } catch {
      // Fall back to the synthesized trace below for malformed historical data.
    }
  }
  // Synthetic fallback for historical records or direct requests without trace_json
  if (log.is_failover && log.failover_from) {
    return [
      {
        index: 1,
        channel_name: log.failover_from,
        channel_id: 0,
        endpoint: '',
        model: log.model,
        attempt: 1,
        status_code: 502,
        latency_ms: 0,
        error: '前序供应商请求失败/超时/熔断，触发故障轮转',
        succeeded: false,
      },
      {
        index: 2,
        channel_name: log.channel_name,
        channel_id: log.channel_id,
        endpoint: log.endpoint || '',
        model: log.upstream_model || log.model,
        attempt: 1,
        status_code: log.status_code,
        latency_ms: log.latency_ms,
        ttft_ms: log.ttft_ms,
        error: log.error_message,
        succeeded: log.status_code >= 200 && log.status_code < 300,
      }
    ]
  }
  return [
    {
      index: 1,
      channel_name: log.channel_name || '默认渠道',
      channel_id: log.channel_id,
      endpoint: log.endpoint || '',
      model: log.upstream_model || log.model,
      attempt: 1,
      status_code: log.status_code,
      latency_ms: log.latency_ms,
      ttft_ms: log.ttft_ms,
      error: log.error_message,
      succeeded: log.status_code >= 200 && log.status_code < 300,
    }
  ]
}

function getLogDiagnostic(log: DesktopRequestLog): {
  badge: 'success' | 'amber' | 'danger' | 'info'
  codeText: string
  title: string
  message: string
} {
  if (log.status_code === 0) {
    return {
      badge: 'info',
      codeText: '流式生成中',
      title: 'SSE 长连接实时推流中',
      message: '客户端已与本地网关建立连接，上游正在持续推流 Token 中。',
    }
  }
  if (log.is_failover && log.status_code >= 200 && log.status_code < 300) {
    return {
      badge: 'amber',
      codeText: '故障轮转 200',
      title: '前序渠道熔断/故障，已平滑故障轮转成功',
      message: `请求在前置渠道【${log.failover_from || '前序渠道'}】未能成功响应，HeiGate 自动毫秒级故障轮转至渠道【${log.channel_name}】并成功完成请求。客户端无感。`,
    }
  }
  if (log.finish_reason === 'client_abort' || (log.status_code >= 200 && log.status_code < 300 && log.error_message?.includes('客户端'))) {
    return {
      badge: 'success',
      codeText: '200 OK (完成)',
      title: '流式生成完成 (客户端主动断开)',
      message: `上游模型已成功生成 ${log.completion_tokens ?? 0} 个 Token。客户端（如 Cursor / Cline）在接收到所需内容或 [DONE] 后正常断开连接，全链路记录为 200 成功。`,
    }
  }
  if (log.status_code >= 200 && log.status_code < 300) {
    return {
      badge: 'success',
      codeText: `${log.status_code} OK`,
      title: '请求执行成功',
      message: `上游渠道【${log.channel_name}】响应正常，全链路耗时 ${log.latency_ms}ms，首字耗时 ${log.ttft_ms || log.latency_ms}ms。`,
    }
  }
  if (log.status_code === 429) {
    return {
      badge: 'danger',
      codeText: '429 限流',
      title: '上游触发速率或配额限制',
      message: log.error_message
        ? `上游回包异常：${log.error_message}`
        : `渠道【${log.channel_name}】触发并发或请求频率限制。建议添加备选渠道以启用自动轮转。`,
    }
  }
  return {
    badge: 'danger',
    codeText: `${log.status_code} 异常`,
    title: `上游请求异常 (HTTP ${log.status_code})`,
    message: log.error_message
      ? `上游返回明确错误：${log.error_message}`
      : `请求未能成功完成 (HTTP ${log.status_code})，请检查渠道地址、API Key 或模型名称是否正确。`,
  }
}

function calculateTokenSpeed(log: DesktopRequestLog): string {
  if (log.completion_tokens && log.latency_ms && log.latency_ms > 0) {
    const speed = (log.completion_tokens / (log.latency_ms / 1000)).toFixed(1)
    return `${speed} t/s`
  }
  return '-'
}

function updateOverviewEvents() {
  events.value = desktopLogs.value.slice(0, 8).map((log) => ({
    time: formatLogTime(log.created_at),
    model: log.model || '通用模型',
    channel: log.is_failover && log.failover_from ? `${log.failover_from} ➔ ${log.channel_name}` : log.channel_name || '默认',
    result: log.status_code === 0 ? '传输中' : log.is_failover ? '轮转成功' : log.status_code >= 200 && log.status_code < 300 ? '200 OK' : `异常 (${log.status_code})`,
    detail: `${log.latency_ms}ms`,
    tone: log.status_code === 0 ? 'badge-sky' : log.is_failover ? 'badge-amber' : log.status_code >= 200 && log.status_code < 300 ? 'badge-emerald' : 'badge-rose',
  }))
}

function startLogsStream() {
  if (logsUnsubscribe) return
  logsUnsubscribe = subscribeDesktopLogs({
    onConnected: () => {
      logsStreamConnected.value = true
    },
    onCreated: (newLog) => {
      logsStreamConnected.value = true
      const idx = desktopLogs.value.findIndex((l) => l.id === newLog.id)
      if (idx !== -1) {
        desktopLogs.value[idx] = { ...desktopLogs.value[idx], ...newLog }
      } else {
        desktopLogs.value.unshift(newLog)
        if (desktopLogs.value.length > 200) {
          desktopLogs.value.pop()
        }
      }
      updateOverviewEvents()
      if (autoScrollLogs.value && activeSection.value === 'logs') {
        nextTick(() => {
          if (logsTableWrapRef.value) {
            logsTableWrapRef.value.scrollTo({ top: 0, behavior: 'smooth' })
          }
        })
      }
    },
    onUpdated: (updatedLog) => {
      logsStreamConnected.value = true
      const idx = desktopLogs.value.findIndex((l) => l.id === updatedLog.id)
      if (idx !== -1) {
        desktopLogs.value[idx] = { ...desktopLogs.value[idx], ...updatedLog }
      } else {
        desktopLogs.value.unshift(updatedLog)
        if (desktopLogs.value.length > 200) {
          desktopLogs.value.pop()
        }
      }
      if (activeLogDetail.value && activeLogDetail.value.id === updatedLog.id) {
        activeLogDetail.value = { ...activeLogDetail.value, ...updatedLog }
      }
      updateOverviewEvents()
    },
    onCleared: () => {
      desktopLogs.value = []
      events.value = []
    },
    onPruned: () => {
      void refreshLogs()
    },
    onError: () => {
      logsStreamConnected.value = false
    },
  })
}

function stopLogsStream() {
  if (logsUnsubscribe) {
    logsUnsubscribe()
    logsUnsubscribe = null
    logsStreamConnected.value = false
  }
}

async function runGatewaySelfTest() {
  if (isTestingGateway.value) return
  isTestingGateway.value = true
  try {
    const health = await checkLocalGateway()
    gatewayOnline.value = health.ok
    gatewayLatency.value = health.latency
    if (health.ok) {
      showToast(`网关自测正常！本地服务 (${gatewayHostOnly.value}) 连通，时延 ${health.latency}ms`)
    } else {
      showToast(`网关自测失败：无法连接到本地网关 (${gatewayHostOnly.value})`)
    }
  } catch (err: any) {
    showToast(`网关自测异常: ${err?.message || '未知错误'}`)
  } finally {
    isTestingGateway.value = false
  }
}

function normalizeEndpointUrl(url: string): string {
  const trimmed = url.trim().replace(/\/+$/, '')
  if (!trimmed) return ''
  if (/^https?:\/\//i.test(trimmed)) {
    return trimmed
  }
  return `https://${trimmed}`
}

function modelsFromText(value: string): string[] {
  return [...new Set(value.split(/[\s,;，；]+/).map((item) => item.trim()).filter(Boolean))]
}

type UpstreamModelsPayload = {
  data?: Array<{ id?: string; name?: string } | string>
  models?: Array<{ id?: string; name?: string } | string>
}

async function fetchUpstreamModels(mode: 'new' | 'edit') {
  const url = mode === 'new' ? newChannelUrl.value : editChannelUrl.value
  const apiKey = mode === 'new'
    ? newChannelKey.value
    : editChannelKey.value.includes('***')
      ? editingChannel.value?.apiKey ?? ''
      : editChannelKey.value
  const protocol = mode === 'new' ? newChannelProtocol.value : editChannelProtocol.value
  const setLoading = mode === 'new' ? (value: boolean) => { isFetchingNewModels.value = value } : (value: boolean) => { isFetchingEditModels.value = value }
  if (!url.trim()) {
    showToast('请先填写 Base URL，再获取上游模型')
    return
  }
  setLoading(true)
  try {
    // 优先通过本地网关后端代理请求，避免浏览器 CORS 拦截
    const proxyRes = await fetchDesktopUpstreamModels(url.trim(), apiKey.trim(), protocol)
    if (proxyRes.ok && proxyRes.models && proxyRes.models.length > 0) {
      if (mode === 'new') newChannelModelText.value = proxyRes.models.join('\n')
      else editChannelModelText.value = proxyRes.models.join('\n')
      showToast(`已获取 ${proxyRes.models.length} 个上游模型`)
      return
    }

    if (!proxyRes.ok && proxyRes.error && !proxyRes.error.includes('无法连接到本地网关服务')) {
      showToast(`获取失败: ${proxyRes.error}`)
      return
    }

    // 备选方案：前端直连拉取（适用于支持 CORS 的第三方或开发直连模式）
    const candidateUrls = [
      `${normalizeEndpointUrl(url)}/models`,
      `${normalizeEndpointUrl(url)}/v1/models`,
    ]
    let fetchedModels: string[] = []
    let lastErrMsg = ''
    for (const targetUrl of candidateUrls) {
      try {
        const response = await fetch(targetUrl, {
          headers: {
            Accept: 'application/json',
            ...(apiKey && !apiKey.includes('***') ? { Authorization: `Bearer ${apiKey}`, 'x-api-key': apiKey } : {}),
          },
          signal: AbortSignal.timeout(6000),
        })
        if (response.ok) {
          const payload = await response.json() as UpstreamModelsPayload
          const items = payload.data ?? payload.models ?? []
          fetchedModels = modelsFromText(items.map((item) => typeof item === 'string' ? item : item.id ?? item.name ?? '').join('\n'))
          if (fetchedModels.length > 0) break
        } else {
          let errDetail = `HTTP ${response.status}`
          try {
            const errPayload = await response.json() as any
            if (errPayload?.error?.message) {
              errDetail = `${errPayload.error.message} (${errDetail})`
            } else if (errPayload?.message) {
              errDetail = `${errPayload.message} (${errDetail})`
            }
          } catch {
            // Preserve the HTTP status when the error response is not JSON.
          }
          if (response.status === 401 || response.status === 403) {
            lastErrMsg = `上游认证失败: ${errDetail}，请检查 API Key 是否正确`
          } else if (response.status === 404) {
            lastErrMsg = `未找到模型列表接口 (${errDetail})`
          } else {
            lastErrMsg = `上游返回 ${errDetail}`
          }
        }
      } catch (err: any) {
        lastErrMsg = err?.message || '网络连接错误'
      }
    }

    if (fetchedModels.length > 0) {
      if (mode === 'new') newChannelModelText.value = fetchedModels.join('\n')
      else editChannelModelText.value = fetchedModels.join('\n')
      showToast(`已获取 ${fetchedModels.length} 个上游模型`)
    } else {
      showToast(proxyRes.error ? `获取失败: ${proxyRes.error}` : (lastErrMsg ? `获取失败: ${lastErrMsg}` : '获取上游模型失败，请检查 Base URL 或网络'))
    }
  } catch (err: any) {
    showToast(`获取上游模型失败: ${err?.message || '未知错误'}`)
  } finally {
    setLoading(false)
  }
}

async function addChannel() {
  const rawUrl = newChannelUrl.value.trim()
  const name = newChannelName.value.trim()
  const modelNames = modelsFromText(newChannelModelText.value)
  const model = modelNames[0] ?? ''
  const interval = newChannelProbePreset.value === -1 ? Number(newChannelProbeCustom.value) : newChannelProbePreset.value
  if (!name || !rawUrl || !newChannelProtocol.value || !model) {
    showToast('请完整填写渠道名称、接口协议与主模型')
    return
  }
  if (newChannelProbePreset.value === -1 && (interval < 15 || isNaN(interval))) {
    showToast('自定义探活间隔必须大于等于 15 秒')
    return
  }
  const finalUrl = normalizeEndpointUrl(rawUrl)
  const draft: Channel = {
    name,
    url: finalUrl,
    apiKey: newChannelKey.value.trim(),
    protocol: newChannelProtocol.value,
    status: 'offline',
    latency: null,
    models: modelNames.length,
    priority: channels.value.length + 1,
    lastCheck: interval <= 0 ? '仅手动探活' : '待探活',
    accent: newChannelProtocol.value === 'anthropic' ? '#f59e0b' : newChannelProtocol.value === 'openai-responses' ? '#3b82f6' : '#10b981',
    enabled: newChannelEnabled.value,
    probeInterval: interval,
    modelNames,
  }
  if (desktopApiActive.value) {
    const created = await createDesktopChannel({ ...toDesktopChannel(draft), apiKey: newChannelKey.value.trim() })
    if (created) {
      channels.value.unshift(fromDesktopChannel(created))
      showToast(`渠道“${name}”创建成功`)
    } else {
      desktopApiActive.value = false
      channels.value.unshift(draft)
      showToast(`渠道“${name}”保存在本地存储`)
    }
  } else {
    channels.value.unshift(draft)
    showToast(`渠道“${name}”已保存`)
  }
  newChannelName.value = ''
  newChannelUrl.value = ''
  newChannelKey.value = ''
  newChannelModelText.value = 'gpt-4o-mini'
  newChannelProtocol.value = 'openai-compatible'
  showAddChannel.value = false
  newChannelEnabled.value = true
  newChannelProbePreset.value = 0
  activeSection.value = 'channels'
}

let isRefreshingLogs = false
async function refreshLogs() {
  if (isRefreshingLogs || !desktopApiActive.value) return
  isRefreshingLogs = true
  try {
    const logs = await listDesktopLogs(100)
    if (logs) {
      desktopLogs.value = logs
      updateOverviewEvents()
    }
  } finally {
    isRefreshingLogs = false
  }
}

async function refreshSystemInfo() {
  if (desktopApiActive.value) {
    const info = await getDesktopSystemInfo()
    if (info) {
      systemInfo.value = info
      logRetentionDays.value = info.log_retention_days
      logMaxCount.value = info.log_max_count ?? 5000
      routingStrategy.value = info.routing_strategy
    }
  }
}

async function setRoutingStrategy(strategy: GlobalRouteStrategy) {
  const previous = routingStrategy.value
  routingStrategy.value = strategy
  if (desktopApiActive.value) {
    const ok = await updateDesktopSettings({ routing_strategy: strategy })
    if (ok) {
      showToast('网关路由策略已更新')
    } else {
      routingStrategy.value = previous
      showToast('网关路由策略更新失败')
    }
  }
}

async function handleSetLogRetention(days: number) {
  logRetentionDays.value = days
  if (desktopApiActive.value) {
    const ok = await updateDesktopSettings({ log_retention_days: days })
    if (ok) {
      showToast(days > 0 ? `日志保留时长已设为 ${days} 天` : '已设为永久保留日志')
      await refreshSystemInfo()
    }
  }
}

async function handleSetLogMaxCount(count: number) {
  logMaxCount.value = count
  if (desktopApiActive.value) {
    const ok = await updateDesktopSettings({ log_max_count: count })
    if (ok) {
      showToast(count > 0 ? `日志容量上限已设为 ${count.toLocaleString()} 条` : '已设为不限制流水条数')
      await refreshSystemInfo()
    }
  }
}

async function handlePruneLogs() {
  isPruningLogs.value = true
  try {
    const res = await pruneDesktopLogs()
    if (res && res.deleted > 0) {
      showToast(`已按策略修剪清理 ${res.deleted} 条旧日志`)
    } else {
      showToast('当前流水均符合保留策略，无需清理')
    }
    await refreshSystemInfo()
  } finally {
    isPruningLogs.value = false
  }
}

function handleClearLogs() {
  confirmModal.value = {
    title: '清空调用流水确认',
    message: '确定清空所有本地网关请求流水日志吗？',
    description: '清空后历史全链路请求流水将无法找回。',
    danger: true,
    confirmText: '清空流水',
    onConfirm: async () => {
      isClearingLogs.value = true
      try {
        const ok = await clearDesktopLogs()
        if (ok) {
          desktopLogs.value = []
          events.value = []
          showToast('已清空全部本地流水日志')
          await refreshSystemInfo()
        }
      } finally {
        isClearingLogs.value = false
      }
    },
  }
}

async function copySystemPath(pathText: string, label: string) {
  try {
    await navigator.clipboard.writeText(pathText)
    showToast(`已复制${label}路径`)
  } catch {
    showToast('复制失败')
  }
}

function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
}

function formatLogTime(isoString: string): string {
  if (!isoString) return '—'
  try {
    const d = new Date(isoString)
    if (isNaN(d.getTime())) return isoString
    const pad = (n: number) => String(n).padStart(2, '0')
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  } catch {
    return isoString
  }
}

function formatLogDateTime(isoString: string): string {
  if (!isoString) return '—'
  try {
    const d = new Date(isoString)
    if (isNaN(d.getTime())) return isoString
    const pad = (n: number) => String(n).padStart(2, '0')
    const year = d.getFullYear()
    const month = pad(d.getMonth() + 1)
    const day = pad(d.getDate())
    const hours = pad(d.getHours())
    const minutes = pad(d.getMinutes())
    const seconds = pad(d.getSeconds())
    return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}`
  } catch {
    return isoString
  }
}

function formatLastCheckTime(str?: string): string {
  if (!str || str === '尚未探活' || str === '待探活' || str === '仅手动探活') return str || '尚未探活'
  try {
    const d = new Date(str)
    if (isNaN(d.getTime())) return str
    const pad = (n: number) => String(n).padStart(2, '0')
    const year = d.getFullYear()
    const month = pad(d.getMonth() + 1)
    const day = pad(d.getDate())
    const hours = pad(d.getHours())
    const minutes = pad(d.getMinutes())
    const seconds = pad(d.getSeconds())
    return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}`
  } catch {
    return str
  }
}

function formatDurationPill(ms?: number): string {
  if (ms === undefined || ms === null || ms < 0) return '0 ms'
  if (ms < 1000) return `${ms} ms`
  return `${(ms / 1000).toFixed(1)} s`
}

function getCompactFailoverNodes(str?: string): string[] {
  if (!str) return []
  const parts = str.split(/\s*[→➔]\s*/)
  return parts.map((part) => {
    const s = part.trim()
    const m = s.match(/^(.+?)\s*(?:响应|异常)?\s*\(([^)]+)\)$/)
    if (m) {
      const name = m[1].trim()
      const reason = m[2].trim()
      const codeMatch = reason.match(/^(\d{3})/)
      if (codeMatch) {
        return `${name} ${codeMatch[1]}`
      }
      return `${name} (${reason.slice(0, 8)})`
    }
    if (s.includes('重试成功')) {
      const name = s.replace(/\s*首次失败.*/, '').trim()
      return `${name} (重试)`
    }
    return s
  })
}

async function copyGatewayUrl() {
  try {
    await navigator.clipboard.writeText(gatewayBaseUrl.value)
    markCopied('url')
    showToast('已复制本地网关 Base URL！')
  } catch {
    showToast('复制失败，请手动复制')
  }
}

async function copyApiKey() {
  try {
    const key = gatewayApiKey.value || await getDesktopGatewayKey()
    if (!key) {
      showToast('本地网关尚未生成有效密钥')
      return
    }
    await navigator.clipboard.writeText(key)
    markCopied('key')
    showToast('已复制本地 API Key！')
  } catch {
    showToast('复制失败')
  }
}

function onToolbarMouseDown(event: MouseEvent) {
  if (event.button !== 0) return
  const target = event.target as HTMLElement | null
  if (target?.closest('button, input, a, select, textarea, [data-no-drag]')) {
    return
  }
  const webkit = (window as unknown as { webkit?: { messageHandlers?: { dragWindow?: { postMessage: (msg: string) => void } } } }).webkit
  if (webkit?.messageHandlers?.dragWindow) {
    webkit.messageHandlers.dragWindow.postMessage('drag')
  }
}

function onToolbarDoubleClick(event: MouseEvent) {
  const target = event.target as HTMLElement | null
  if (target?.closest('button, input, a, select, textarea, [data-no-drag]')) {
    return
  }
  const webkit = (window as unknown as { webkit?: { messageHandlers?: { zoomWindow?: { postMessage: (msg: string) => void } } } }).webkit
  if (webkit?.messageHandlers?.zoomWindow) {
    webkit.messageHandlers.zoomWindow.postMessage('zoom')
  }
}

let backdropMouseDownPos = { x: 0, y: 0 }

function onBackdropMouseDown(event: MouseEvent) {
  if (event.button !== 0) return
  if (event.target !== event.currentTarget) return
  backdropMouseDownPos = { x: event.screenX, y: event.screenY }

  const webkit = (window as unknown as { webkit?: { messageHandlers?: { dragWindow?: { postMessage: (msg: string) => void } } } }).webkit
  if (webkit?.messageHandlers?.dragWindow) {
    webkit.messageHandlers.dragWindow.postMessage('drag')
  }
}

function handleBackdropClick(event: MouseEvent, closeCallback: () => void) {
  if (event.target !== event.currentTarget) return
  const dx = Math.abs(event.screenX - backdropMouseDownPos.x)
  const dy = Math.abs(event.screenY - backdropMouseDownPos.y)
  if (dx > 5 || dy > 5) {
    return
  }
  closeCallback()
}

function handleContextMenu(event: MouseEvent) {
  const target = event.target as HTMLElement | null
  const isEditable = target && (
    target.tagName === 'INPUT' ||
    target.tagName === 'TEXTAREA' ||
    target.isContentEditable ||
    Boolean(target.closest('input, textarea, [contenteditable="true"]'))
  )
  if (!isEditable) {
    event.preventDefault()
  }
}
</script>

<template>
  <div class="m26-window" @contextmenu="handleContextMenu">
    <!-- Background Ambient Glow Canvas -->
    <div class="m26-ambient-layer">
      <div class="m26-glow-orb orb-1"></div>
      <div class="m26-glow-orb orb-2"></div>
      <div class="m26-glow-orb orb-3"></div>
    </div>

    <!-- TOP FLOATING PILL CAPSULE NAVIGATION BAR (macOS 26 White) -->
    <header ref="topNavbarRef" class="m26-top-navbar" @mousedown="onToolbarMouseDown" @dblclick="onToolbarDoubleClick">
      <!-- Left: macOS window buttons spacer -->
      <div class="m26-nav-left" data-no-drag>
        <div class="m26-traffic-lights" v-if="!isDesktop" aria-hidden="true">
          <span class="light red"></span>
          <span class="light yellow"></span>
          <span class="light green"></span>
        </div>
        <div class="m26-native-titlebar-spacer" v-else></div>
      </div>

      <!-- Center: FLOATING PILL CAPSULE NAV (Flex centered & Auto collapse text when constrained) -->
      <div class="m26-nav-center" ref="navCenterRef" data-no-drag>
        <nav class="m26-floating-nav" :class="{ 'compact-tabs': isCompactTabs }">
          <!-- 1. 总览 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'overview' }"
            @click="activeSection = 'overview'"
            title="总览"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <path d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 18a8 8 0 1 1 8-8 8 8 0 0 1-8 8z"/>
              <path d="M12 6v6l4 2"/>
            </svg>
            <span class="m26-pill-text">总览</span>
          </button>

          <!-- 看板 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'analytics' }"
            @click="activeSection = 'analytics'"
            title="数据看板"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <path d="M18 20V10"/>
              <path d="M12 20V4"/>
              <path d="M6 20v-6"/>
            </svg>
            <span class="m26-pill-text">看板</span>
          </button>

          <!-- 2. 供应商 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'channels' }"
            @click="activeSection = 'channels'"
            :title="'供应商' + (channels.length ? ` (${channels.length})` : '')"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <rect x="2" y="3" width="20" height="7" rx="2" ry="2"/>
              <rect x="2" y="14" width="20" height="7" rx="2" ry="2"/>
              <line x1="6" y1="6.5" x2="6.01" y2="6.5"/>
              <line x1="6" y1="17.5" x2="6.01" y2="17.5"/>
            </svg>
            <span class="m26-pill-text">供应商</span>
            <span v-if="channels.length" class="m26-pill-badge">{{ channels.length }}</span>
          </button>

          <!-- 3. 模型 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'models' }"
            @click="activeSection = 'models'"
            :title="'模型' + (models.length ? ` (${models.length})` : '')"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <polygon points="12 2 2 7 12 12 22 7 12 2"/>
              <polyline points="2 17 12 22 22 17"/>
              <polyline points="2 12 12 17 22 12"/>
            </svg>
            <span class="m26-pill-text">模型</span>
            <span v-if="models.length" class="m26-pill-badge">{{ models.length }}</span>
          </button>

          <!-- 4. 日志 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'logs' }"
            @click="activeSection = 'logs'"
            title="日志"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <circle cx="12" cy="12" r="10"/>
              <polyline points="12 6 12 12 16 14"/>
            </svg>
            <span class="m26-pill-text">日志</span>
          </button>

          <!-- 5. 设置 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'settings' }"
            @click="activeSection = 'settings'"
            title="设置"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <line x1="4" y1="21" x2="4" y2="14"/>
              <line x1="4" y1="10" x2="4" y2="3"/>
              <line x1="12" y1="21" x2="12" y2="12"/>
              <line x1="12" y1="8" x2="12" y2="3"/>
              <line x1="20" y1="21" x2="20" y2="16"/>
              <line x1="20" y1="12" x2="20" y2="3"/>
              <line x1="1" y1="14" x2="7" y2="14"/>
              <line x1="9" y1="8" x2="15" y2="8"/>
              <line x1="17" y1="16" x2="23" y2="16"/>
            </svg>
            <span class="m26-pill-text">设置</span>
          </button>
        </nav>
      </div>

      <!-- Right: Spacer balancing left window buttons -->
      <div class="m26-nav-right" aria-hidden="true"></div>
    </header>

    <!-- Main Content Workspace (Full Width, No Left Sidebar) -->
    <main class="m26-workspace">
      <!-- Scrollable Main Viewport -->
      <div class="m26-viewport">
        <!-- SECTION 1: OVERVIEW -->
        <section v-if="activeSection === 'overview'" class="m26-section-view">
          <!-- Gateway Quick Test & Connectivity Status & Credentials -->
          <div class="m26-gateway-self-test-card">
            <!-- Row 1: Status & Title & Self Test -->
            <div class="m26-gateway-status-row">
              <div class="flex items-center gap-3">
                <div class="m26-test-pulse" :class="{ offline: !gatewayOnline }"></div>
                <div class="flex items-center gap-2">
                  <h4 class="text-sm font-semibold text-main">本地分发网关</h4>
                  <span class="m26-gateway-status-badge" :class="{ offline: !gatewayOnline }">
                    {{ gatewayOnline ? `健康运行中 · 当前时延 ${gatewayLatency ?? 0}ms` : '网关离线' }}
                  </span>
                </div>
              </div>
              <div class="flex items-center gap-2">
                <button
                  class="m26-glass-btn"
                  :disabled="isTestingGateway"
                  @click="runGatewaySelfTest"
                  title="向本地网关发起一次连通性与模型校验"
                >
                  <span v-if="isTestingGateway" class="m26-spin-dot"></span>
                  <span>{{ isTestingGateway ? '自测中...' : '网关连通性自测' }}</span>
                </button>
              </div>
            </div>

            <!-- Row 2: Credentials (URL & API Key) -->
            <div class="m26-gateway-creds-row">
              <!-- URL Block -->
              <div class="m26-gateway-cred-item">
                <span class="m26-gateway-cred-label">URL</span>
                <span
                  class="m26-gateway-cred-value"
                  @click="copyGatewayUrl"
                  :title="'点击复制 Base URL: ' + gatewayBaseUrl"
                >
                  {{ gatewayBaseUrl }}
                </span>
                <button class="m26-cred-copy-btn" @click="copyGatewayUrl" title="复制 Base URL">
                  <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                  </svg>
                  <span>复制</span>
                </button>
              </div>

              <!-- API Key Block -->
              <div class="m26-gateway-cred-item">
                <span class="m26-gateway-cred-label">API KEY</span>
                <span
                  class="m26-gateway-cred-value"
                  @click="copyApiKey"
                  :title="'点击复制 API Key'"
                >
                  {{ showOverviewApiKey ? (gatewayApiKey || '暂无 Key') : (maskedOverviewApiKey || '••••••••') }}
                </span>
                <div class="m26-cred-actions">
                  <button
                    class="m26-cred-icon-btn"
                    @click="showOverviewApiKey = !showOverviewApiKey"
                    :title="showOverviewApiKey ? '隐藏 API Key' : '显示完整 API Key'"
                  >
                    <svg v-if="!showOverviewApiKey" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                      <circle cx="12" cy="12" r="3"/>
                    </svg>
                    <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
                      <line x1="1" y1="1" x2="23" y2="23"/>
                    </svg>
                  </button>
                  <button class="m26-cred-copy-btn" @click="copyApiKey" title="复制 API Key">
                    <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                      <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                    </svg>
                    <span>复制</span>
                  </button>
                </div>
              </div>
            </div>
          </div>

          <!-- Clients Quick Integration Card (Claude & Codex) -->
          <div class="m26-clients-integration-card">
            <div class="m26-clients-card-header">
              <div class="flex items-center gap-2.5">
                <div class="m26-clients-icon-box">
                  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                    <rect x="2" y="3" width="20" height="14" rx="2" ry="2"/>
                    <line x1="8" y1="21" x2="16" y2="21"/>
                    <line x1="12" y1="17" x2="12" y2="21"/>
                  </svg>
                </div>
                <div>
                  <h4 class="text-sm font-semibold text-main">桌面客户端一键接入与启动</h4>
                  <p class="text-xs text-muted">原生支持 Claude 桌面端与 Codex (ChatGPT) 桌面端，自动双向协议转译，一键配置生效</p>
                </div>
              </div>
              <div class="flex items-center gap-2">
                <button class="m26-glass-btn text-xs" :disabled="isLoadingClients" @click="loadDesktopClients" title="重新检测客户端状态">
                  <span v-if="isLoadingClients" class="m26-spin-dot"></span>
                  <span>{{ isLoadingClients ? '检测中...' : '刷新状态' }}</span>
                </button>
              </div>
            </div>

            <div class="m26-clients-grid">
              <!-- Client 1: Claude Desktop -->
              <div class="m26-client-item" :class="{ 'is-configured': claudeClientInfo?.configured }">
                <div class="m26-client-top">
                  <div class="flex items-center gap-2.5">
                    <div class="m26-client-logo claude">
                      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M12 2v20M2 12h20M4.93 4.93l14.14 14.14M4.93 19.07l14.14-14.14"/>
                      </svg>
                    </div>
                    <div>
                      <div class="flex items-center gap-2">
                        <span class="font-semibold text-sm text-main">Claude 桌面端</span>
                        <span class="m26-app-badge" :class="{ running: claudeClientInfo?.running, installed: claudeClientInfo?.installed }">
                          {{ claudeClientInfo?.running ? '运行中' : (claudeClientInfo?.installed ? '已安装' : '未安装') }}
                        </span>
                      </div>
                      <span class="text-xs text-muted">Anthropic 原生客户端 · 3P 扩展模式</span>
                    </div>
                  </div>
                  <div class="m26-client-badge-pill" :class="{ active: claudeClientInfo?.configured }">
                    {{ claudeClientInfo?.configured ? '已配置网关' : '未接入' }}
                  </div>
                </div>

                <div class="m26-client-body">
                  <div class="m26-client-meta-line">
                    <span class="label">挂载槽位</span>
                    <span class="value">{{ claudeClientInfo?.configured_models?.length ? `${claudeClientInfo.configured_models.length} 个模型已挂载` : '未挂载模型' }}</span>
                  </div>
                  <div class="m26-client-chips" v-if="claudeClientInfo?.configured_models?.length">
                    <span v-for="m in claudeClientInfo.configured_models.slice(0, 3)" :key="m" class="m26-client-chip" :title="m">{{ m }}</span>
                    <span v-if="claudeClientInfo.configured_models.length > 3" class="m26-client-chip more">+{{ claudeClientInfo.configured_models.length - 3 }}</span>
                  </div>
                  <div v-else class="text-xs text-slate-400 py-1">
                    点击“配置模型”挑选要映射给 Claude 桌面端的模型
                  </div>
                </div>

                <div class="m26-client-footer">
                  <button class="m26-glass-btn text-xs flex-1" @click="openClaudeConfigModal">
                    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                      <circle cx="12" cy="12" r="3"/>
                      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
                    </svg>
                    <span>配置模型</span>
                  </button>
                  <button class="m26-launch-btn text-xs flex-1" :disabled="isLaunchingClaude" @click="handleLaunchClaude" title="调起 Claude 桌面端并以 3P 模式加载网关">
                    <span v-if="isLaunchingClaude" class="m26-spin-dot"></span>
                    <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                      <polygon points="5 3 19 12 5 21 5 3"/>
                    </svg>
                    <span>{{ isLaunchingClaude ? '启动中...' : '一键启动' }}</span>
                  </button>
                </div>
              </div>

              <!-- Client 2: Codex Desktop (ChatGPT) -->
              <div class="m26-client-item" :class="{ 'is-configured': codexClientInfo?.configured }">
                <div class="m26-client-top">
                  <div class="flex items-center gap-2.5">
                    <div class="m26-client-logo codex">
                      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                        <polyline points="16 18 22 12 16 6"/>
                        <polyline points="8 6 2 12 8 18"/>
                      </svg>
                    </div>
                    <div>
                      <div class="flex items-center gap-2">
                        <span class="font-semibold text-sm text-main">Codex 桌面端</span>
                        <span class="m26-app-badge" :class="{ running: codexClientInfo?.running, installed: codexClientInfo?.installed }">
                          {{ codexClientInfo?.running ? '运行中' : (codexClientInfo?.installed ? '已安装' : '未安装') }}
                        </span>
                      </div>
                      <span class="text-xs text-muted">ChatGPT 桌面端 / Codex CLI 原生支持</span>
                    </div>
                  </div>
                  <div class="m26-client-badge-pill" :class="{ active: codexClientInfo?.configured }">
                    {{ codexClientInfo?.configured ? '已配置网关' : '未接入' }}
                  </div>
                </div>

                <div class="m26-client-body">
                  <div class="m26-client-meta-line">
                    <span class="label">默认模型</span>
                    <span class="value font-mono text-xs">{{ codexClientInfo?.default_model || '未设定' }}</span>
                  </div>
                  <div class="m26-client-chips" v-if="codexClientInfo?.configured_models?.length">
                    <span v-for="m in codexClientInfo.configured_models.slice(0, 3)" :key="m" class="m26-client-chip" :title="m">{{ m }}</span>
                    <span v-if="codexClientInfo.configured_models.length > 3" class="m26-client-chip more">+{{ codexClientInfo.configured_models.length - 3 }}</span>
                  </div>
                  <div v-else class="text-xs text-slate-400 py-1">
                    点击“配置模型”生成 Codex 专属模型目录
                  </div>
                </div>

                <div class="m26-client-footer">
                  <button class="m26-glass-btn text-xs flex-1" @click="openCodexConfigModal">
                    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                      <circle cx="12" cy="12" r="3"/>
                      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
                    </svg>
                    <span>配置模型</span>
                  </button>
                  <button class="m26-launch-btn text-xs flex-1" :disabled="isLaunchingCodex" @click="handleLaunchCodex" title="调起 ChatGPT.app 或终端启动 Codex">
                    <span v-if="isLaunchingCodex" class="m26-spin-dot"></span>
                    <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                      <polygon points="5 3 19 12 5 21 5 3"/>
                    </svg>
                    <span>{{ isLaunchingCodex ? '启动中...' : '一键启动' }}</span>
                  </button>
                </div>
              </div>
            </div>
          </div>

          <!-- Bento Grid Metrics -->
          <div class="m26-bento-grid">
            <div class="m26-bento-card">
              <div class="m26-bento-head">
                <span class="m26-bento-label">可用渠道比率</span>
                <span class="m26-pill-icon emerald">
                  <svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
                    <circle cx="8" cy="8" r="4" />
                  </svg>
                </span>
              </div>
              <div class="m26-bento-stat">
                <span class="m26-bento-num">{{ healthyChannels }}</span>
                <span class="m26-bento-sub">/ {{ channels.length }}</span>
              </div>
              <div class="m26-bento-foot">
                <span class="m26-tag-subtle">{{ enabledChannels.length }} 个启用调度中</span>
              </div>
            </div>

            <div class="m26-bento-card">
              <div class="m26-bento-head">
                <span class="m26-bento-label">综合响应延迟</span>
                <span class="m26-pill-icon indigo">
                  <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="2 8 5 11 9 4 14 10" />
                  </svg>
                </span>
              </div>
              <div class="m26-bento-stat">
                <span class="m26-bento-num">{{ averageLatency != null ? averageLatency : '—' }}</span>
                <span class="m26-bento-unit" v-if="averageLatency != null">ms</span>
              </div>
              <div class="m26-bento-foot">
                <span class="m26-tag-subtle">{{ averageLatency && averageLatency < 500 ? '极速响应' : '正常负载' }}</span>
              </div>
            </div>

            <div class="m26-bento-card">
              <div class="m26-bento-head">
                <span class="m26-bento-label">分发策略</span>
                <span class="m26-pill-icon amber">
                  <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="8" cy="8" r="6" />
                    <line x1="8" y1="4" x2="8" y2="8" />
                    <line x1="8" y1="8" x2="11" y2="11" />
                  </svg>
                </span>
              </div>
              <div class="m26-bento-stat">
                <span class="m26-bento-text">
                  {{ (routingStrategy === 'smart_quality' || routingStrategy === 'priority') ? '智能综合质量' : routingStrategy === 'latency' ? '最低延迟' : '轮询均衡' }}
                </span>
              </div>
              <div class="m26-bento-foot">
                <span class="m26-tag-subtle">自动故障熔断隔离</span>
              </div>
            </div>

            <div class="m26-bento-card">
              <div class="m26-bento-head">
                <span class="m26-bento-label">大模型总数</span>
                <span class="m26-pill-icon cyan">
                  <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                    <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
                  </svg>
                </span>
              </div>
              <div class="m26-bento-stat">
                <span class="m26-bento-num">{{ models.length }}</span>
                <span class="m26-bento-sub">个聚合模型</span>
              </div>
              <div class="m26-bento-foot">
                <span class="m26-tag-subtle">{{ availableModels }} 家上游提供</span>
              </div>
            </div>
          </div>

          <!-- Quick Actions & Status Strip -->
          <div class="m26-quick-bar">
            <div class="m26-bar-left">
              <button class="m26-glass-btn" @click="probeAllChannels">
                <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M14 8A6 6 0 1 1 8 2" />
                  <polyline points="14 3 14 8 9 8" />
                </svg>
                <span>全网一键探活</span>
              </button>
              <button class="m26-glass-btn" @click="exportConfig">
                <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M8 2v8M4 6l4 4 4-4M2 13h12" />
                </svg>
                <span>导出配置 JSON</span>
              </button>
            </div>
            <div class="m26-bar-right">
              <button class="m26-link-btn" @click="activeSection = 'channels'">查看全部渠道 ➔</button>
            </div>
          </div>

          <!-- Live Channels Status Grid Preview -->
          <div class="m26-card-section">
            <h3 class="m26-section-title">活跃渠道状态矩阵</h3>
            <div class="m26-mini-grid" v-if="channels.length">
              <div
                v-for="c in channels.slice(0, 6)"
                :key="c.name"
                class="m26-mini-card"
                :class="[c.status, { disabled: !c.enabled }]"
                @click="probeChannel(c)"
              >
                <div class="m26-mini-head">
                  <span class="m26-status-dot" :class="c.status"></span>
                  <span class="m26-mini-name">{{ c.name }}</span>
                </div>
                <div class="m26-mini-meta">
                  <span class="m26-mini-proto">{{ c.protocol }}</span>
                  <span class="m26-mini-lat">{{ c.latency ? `${c.latency}ms` : '—' }}</span>
                </div>
              </div>
            </div>
            <div class="m26-empty-state" v-else>
              <p>尚未添加任何渠道，点击上方「添加渠道」开始接入。</p>
            </div>
          </div>
        </section>

        <!-- SECTION: DATA DASHBOARD (KANBAN) -->
        <section v-if="activeSection === 'analytics'" class="m26-section-view">
          <!-- Top Control Header: Title & Time Range Filter -->
          <div class="m26-analytics-header-bar">
            <div>
              <div class="flex items-center gap-2.5">
                <h2 class="text-base font-bold text-main">用量与性能数据看板</h2>
                <span class="m26-badge emerald text-xs font-mono" v-if="analyticsData?.summary?.total_requests">
                  共 {{ analyticsData.summary.total_requests }} 次调用记录
                </span>
              </div>
              <p class="text-xs text-muted mt-0.5">全维度透视模型调用量、Token 消耗、响应时延与首字 (TTFT) 表现</p>
            </div>

            <div class="flex items-center gap-3">
              <!-- Time Range Selector -->
              <div class="m26-time-pills">
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 1 }"
                  @click="setAnalyticsDays(1)"
                >今日</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 7 }"
                  @click="setAnalyticsDays(7)"
                >近 7 天</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 14 }"
                  @click="setAnalyticsDays(14)"
                >近 14 天</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 30 }"
                  @click="setAnalyticsDays(30)"
                >近 30 天</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 0 }"
                  @click="setAnalyticsDays(0)"
                >全部历史</button>
              </div>

              <!-- Refresh Button -->
              <button
                class="m26-glass-btn text-xs flex items-center gap-1.5"
                :disabled="isLoadingAnalytics"
                @click="loadAnalytics"
                title="刷新看板数据"
              >
                <span v-if="isLoadingAnalytics" class="m26-spin-dot"></span>
                <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M23 4v6h-6M1 20v-6h6"/>
                  <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>
                </svg>
                <span>刷新</span>
              </button>
            </div>
          </div>

          <!-- KPI Summary Cards Grid (5 Cards) -->
          <div class="m26-analytics-kpi-grid">
            <!-- Card 1: Total Calls -->
            <div class="m26-bento-card m26-kpi-card">
              <div class="flex items-center justify-between">
                <span class="m26-kpi-label">调用请求总量</span>
                <div class="m26-kpi-icon-badge blue">
                  <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M18 20V10M12 20V4M6 20v-6"/>
                  </svg>
                </div>
              </div>
              <div class="m26-kpi-value-row">
                <span class="m26-kpi-big-num">{{ analyticsData?.summary?.total_requests ?? 0 }}</span>
                <span class="m26-kpi-unit">次</span>
              </div>
              <div class="m26-kpi-footer">
                <span class="m26-kpi-badge emerald">成功 {{ analyticsData?.summary?.success_requests ?? 0 }}</span>
                <span class="m26-kpi-badge rose" v-if="analyticsData?.summary?.failed_requests">失败 {{ analyticsData?.summary?.failed_requests }}</span>
                <span class="text-xs text-muted" v-else>无失败请求</span>
              </div>
            </div>

            <!-- Card 2: Total Tokens -->
            <div class="m26-bento-card m26-kpi-card">
              <div class="flex items-center justify-between">
                <span class="m26-kpi-label">Token 消耗总计</span>
                <div class="m26-kpi-icon-badge purple">
                  <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/>
                  </svg>
                </div>
              </div>
              <div class="m26-kpi-value-row">
                <span class="m26-kpi-big-num">{{ formatAnalyticsTokens(analyticsData?.summary?.total_tokens ?? 0) }}</span>
                <span class="m26-kpi-unit">tokens</span>
              </div>
              <div class="m26-kpi-footer">
                <span class="text-xs text-muted">提 {{ formatAnalyticsTokens(analyticsData?.summary?.prompt_tokens ?? 0) }} · 补 {{ formatAnalyticsTokens(analyticsData?.summary?.completion_tokens ?? 0) }}</span>
              </div>
            </div>

            <!-- Card 3: Avg Latency -->
            <div class="m26-bento-card m26-kpi-card">
              <div class="flex items-center justify-between">
                <span class="m26-kpi-label">平均端到端耗时</span>
                <div class="m26-kpi-icon-badge amber">
                  <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="12" cy="12" r="10"/>
                    <polyline points="12 6 12 12 16 14"/>
                  </svg>
                </div>
              </div>
              <div class="m26-kpi-value-row">
                <span class="m26-kpi-big-num">{{ formatAnalyticsLatency(analyticsData?.summary?.avg_latency_ms ?? 0) }}</span>
              </div>
              <div class="m26-kpi-footer">
                <span class="text-xs text-muted">包含推理与网络往返</span>
              </div>
            </div>

            <!-- Card 4: Avg TTFT -->
            <div class="m26-bento-card m26-kpi-card">
              <div class="flex items-center justify-between">
                <span class="m26-kpi-label">首字平均用时 (TTFT)</span>
                <div class="m26-kpi-icon-badge cyan">
                  <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2">
                    <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>
                  </svg>
                </div>
              </div>
              <div class="m26-kpi-value-row">
                <span class="m26-kpi-big-num">{{ formatAnalyticsLatency(analyticsData?.summary?.avg_ttft_ms ?? 0) }}</span>
              </div>
              <div class="m26-kpi-footer">
                <span class="text-xs text-muted">首包响应时间 (流式感知)</span>
              </div>
            </div>

            <!-- Card 5: Success Rate -->
            <div class="m26-bento-card m26-kpi-card">
              <div class="flex items-center justify-between">
                <span class="m26-kpi-label">请求成功率</span>
                <div class="m26-kpi-icon-badge emerald">
                  <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/>
                    <polyline points="22 4 12 14.01 9 11.01"/>
                  </svg>
                </div>
              </div>
              <div class="m26-kpi-value-row">
                <span class="m26-kpi-big-num">{{ (analyticsData?.summary?.success_rate ?? 100).toFixed(1) }}</span>
                <span class="m26-kpi-unit">%</span>
              </div>
              <div class="m26-kpi-footer">
                <span class="m26-kpi-badge" :class="(analyticsData?.summary?.success_rate ?? 100) >= 95 ? 'emerald' : 'amber'">
                  {{ (analyticsData?.summary?.success_rate ?? 100) >= 95 ? '服务健康稳定' : '存在调用报错' }}
                </span>
              </div>
            </div>
          </div>

          <!-- Charts Row: Daily Requests & Daily Tokens -->
          <div class="m26-analytics-charts-grid">
            <!-- Chart 1: Daily Requests -->
            <div class="m26-analytics-card">
              <div class="m26-card-header-sub">
                <div class="flex items-center gap-2">
                  <div class="m26-card-sub-icon blue"></div>
                  <h3 class="text-sm font-semibold text-main">每日请求量走势</h3>
                </div>
                <div class="flex items-center gap-3 text-xs text-muted">
                  <span class="flex items-center gap-1.5"><span class="m26-legend-dot emerald"></span>成功</span>
                  <span class="flex items-center gap-1.5"><span class="m26-legend-dot rose"></span>失败</span>
                </div>
              </div>

              <div class="m26-chart-container" v-if="analyticsData?.daily && analyticsData.daily.length > 0">
                <div class="m26-chart-bars-wrap">
                  <div
                    v-for="d in analyticsData.daily"
                    :key="d.date"
                    class="m26-bar-column-group"
                  >
                    <!-- Tooltip Popup -->
                    <div class="m26-bar-tooltip">
                      <div class="font-semibold text-xs text-main mb-1">{{ d.date }}</div>
                      <div class="text-xs text-muted flex justify-between gap-4">
                        <span>总请求:</span>
                        <b class="text-main">{{ d.total_requests }} 次</b>
                      </div>
                      <div class="text-xs text-muted flex justify-between gap-4">
                        <span>成功:</span>
                        <span class="text-emerald-600 font-semibold">{{ d.success_requests }} 次</span>
                      </div>
                      <div class="text-xs text-muted flex justify-between gap-4" v-if="d.failed_requests">
                        <span>失败:</span>
                        <span class="text-rose-600 font-semibold">{{ d.failed_requests }} 次</span>
                      </div>
                      <div class="text-xs text-muted flex justify-between gap-4 mt-1 pt-1 border-t border-slate-100">
                        <span>均延 / 首字:</span>
                        <span class="font-mono text-main">{{ formatAnalyticsLatency(d.avg_latency_ms) }} / {{ formatAnalyticsLatency(d.avg_ttft_ms) }}</span>
                      </div>
                    </div>

                    <!-- Bar Track -->
                    <div class="m26-bar-track">
                      <div
                        class="m26-bar-fill failed"
                        v-if="d.failed_requests"
                        :style="{ height: Math.max((d.failed_requests / maxDailyRequests) * 100, 5) + '%' }"
                      ></div>
                      <div
                        class="m26-bar-fill success"
                        :style="{ height: Math.max((d.success_requests / maxDailyRequests) * 100, 6) + '%' }"
                      ></div>
                    </div>
                    <!-- X-Axis Label -->
                    <span class="m26-bar-label">{{ d.date.length >= 10 ? d.date.slice(5) : d.date }}</span>
                  </div>
                </div>
              </div>
              <div class="m26-analytics-empty" v-else>
                <span>所选时间范围内暂无调用记录</span>
              </div>
            </div>

            <!-- Chart 2: Daily Tokens -->
            <div class="m26-analytics-card">
              <div class="m26-card-header-sub">
                <div class="flex items-center gap-2">
                  <div class="m26-card-sub-icon purple"></div>
                  <h3 class="text-sm font-semibold text-main">每日 Token 消耗走势</h3>
                </div>
                <div class="flex items-center gap-3 text-xs text-muted">
                  <span class="flex items-center gap-1.5"><span class="m26-legend-dot purple"></span>提示 Prompt</span>
                  <span class="flex items-center gap-1.5"><span class="m26-legend-dot cyan"></span>生成 Completion</span>
                </div>
              </div>

              <div class="m26-chart-container" v-if="analyticsData?.daily && analyticsData.daily.length > 0">
                <div class="m26-chart-bars-wrap">
                  <div
                    v-for="d in analyticsData.daily"
                    :key="d.date"
                    class="m26-bar-column-group"
                  >
                    <!-- Tooltip Popup -->
                    <div class="m26-bar-tooltip">
                      <div class="font-semibold text-xs text-main mb-1">{{ d.date }}</div>
                      <div class="text-xs text-muted flex justify-between gap-4">
                        <span>总 Tokens:</span>
                        <b class="text-main">{{ formatAnalyticsTokens(d.total_tokens) }}</b>
                      </div>
                      <div class="text-xs text-muted flex justify-between gap-4">
                        <span>提示 Prompt:</span>
                        <span class="text-indigo-600 font-semibold">{{ formatAnalyticsTokens(d.prompt_tokens) }}</span>
                      </div>
                      <div class="text-xs text-muted flex justify-between gap-4">
                        <span>补全 Completion:</span>
                        <span class="text-cyan-600 font-semibold">{{ formatAnalyticsTokens(d.completion_tokens) }}</span>
                      </div>
                    </div>

                    <!-- Bar Track (Stacked) -->
                    <div class="m26-bar-track">
                      <div
                        class="m26-bar-fill purple"
                        :style="{ height: Math.max((d.prompt_tokens / maxDailyTokens) * 100, 3) + '%' }"
                      ></div>
                      <div
                        class="m26-bar-fill cyan"
                        :style="{ height: Math.max((d.completion_tokens / maxDailyTokens) * 100, 3) + '%' }"
                      ></div>
                    </div>
                    <!-- X-Axis Label -->
                    <span class="m26-bar-label">{{ d.date.length >= 10 ? d.date.slice(5) : d.date }}</span>
                  </div>
                </div>
              </div>
              <div class="m26-analytics-empty" v-else>
                <span>所选时间范围内暂无调用记录</span>
              </div>
            </div>
          </div>

          <!-- Section 3: Per-Model Breakdown Table -->
          <div class="m26-analytics-card">
            <div class="m26-card-header-sub">
              <div>
                <h3 class="text-sm font-semibold text-main">各模型用量与时延表现明细</h3>
                <p class="text-xs text-muted mt-0.5">按调用量降序分析各模型的 Token 规模、端到端延迟、首字 TTFT 与首选承载渠道</p>
              </div>

              <!-- Search filter in card header -->
              <div class="m26-model-filter-input-wrap">
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                  <circle cx="6.5" cy="6.5" r="4.5"/>
                  <line x1="10" y1="10" x2="14" y2="14"/>
                </svg>
                <input
                  type="text"
                  v-model="analyticsModelSearch"
                  placeholder="搜索模型或承载渠道..."
                  class="m26-sub-search-input"
                />
              </div>
            </div>

            <!-- Table -->
            <div class="m26-analytics-table-wrap" v-if="filteredAnalyticsModels.length > 0">
              <table class="m26-analytics-table">
                <thead>
                  <tr>
                    <th class="text-left" style="width: 25%;">模型名称</th>
                    <th class="text-left" style="width: 18%;">调用频次 & 占比</th>
                    <th class="text-left" style="width: 18%;">Token 消耗 (总 / 提 / 补)</th>
                    <th class="text-left" style="width: 13%;">平均耗时</th>
                    <th class="text-left" style="width: 12%;">首字用时</th>
                    <th class="text-left" style="width: 14%;">时延范围 (最小~最大)</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="m in filteredAnalyticsModels" :key="m.model" class="m26-analytics-row">
                    <!-- Model name & top channel -->
                    <td>
                      <div class="flex items-center gap-2.5">
                        <div class="m26-model-icon-dot"></div>
                        <div>
                          <div class="font-semibold text-xs text-main font-mono">{{ m.model }}</div>
                          <div class="text-[10.5px] text-muted flex items-center gap-1 mt-0.5" v-if="m.top_channel">
                            <span>主承载:</span>
                            <span class="m26-channel-tag">{{ m.top_channel }}</span>
                          </div>
                        </div>
                      </div>
                    </td>

                    <!-- Requests & Share -->
                    <td>
                      <div>
                        <div class="flex items-center justify-between text-xs font-mono mb-1">
                          <span class="text-main font-semibold">{{ m.total_requests }} 次</span>
                          <span class="text-muted text-[11px]">{{ ((m.total_requests / (analyticsData?.summary?.total_requests || 1)) * 100).toFixed(1) }}%</span>
                        </div>
                        <div class="m26-mini-progress-bar">
                          <div
                            class="m26-mini-progress-fill"
                            :style="{ width: Math.min(((m.total_requests / (analyticsData?.summary?.total_requests || 1)) * 100), 100) + '%' }"
                          ></div>
                        </div>
                      </div>
                    </td>

                    <!-- Tokens -->
                    <td>
                      <div class="text-xs">
                        <div class="font-semibold text-main font-mono">{{ formatAnalyticsTokens(m.total_tokens) }}</div>
                        <div class="text-[10.5px] text-muted font-mono mt-0.5">
                          提 {{ formatAnalyticsTokens(m.prompt_tokens) }} · 补 {{ formatAnalyticsTokens(m.completion_tokens) }}
                        </div>
                      </div>
                    </td>

                    <!-- Avg Latency -->
                    <td>
                      <span class="m26-latency-pill font-mono" :class="getLatencyTagClass(m.avg_latency_ms)">
                        {{ formatAnalyticsLatency(m.avg_latency_ms) }}
                      </span>
                    </td>

                    <!-- TTFT -->
                    <td>
                      <span class="text-xs font-mono font-medium text-main">
                        {{ formatAnalyticsLatency(m.avg_ttft_ms) }}
                      </span>
                    </td>

                    <!-- Min ~ Max -->
                    <td>
                      <span class="text-xs font-mono text-muted">
                        {{ formatAnalyticsLatency(m.min_latency_ms) }} ~ {{ formatAnalyticsLatency(m.max_latency_ms) }}
                      </span>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
            <div class="m26-analytics-empty" v-else>
              <span>无匹配的模型记录</span>
            </div>
          </div>

          <!-- Section 4: Upstream Channel Performance Matrix -->
          <div class="m26-analytics-card" v-if="analyticsData?.channels && analyticsData.channels.length > 0">
            <div class="m26-card-header-sub">
              <div>
                <h3 class="text-sm font-semibold text-main">上游供应商渠道调度与承载分布</h3>
                <p class="text-xs text-muted mt-0.5">统计网关向各个渠道分发请求的吞吐量、平均响应耗时及成功率表现</p>
              </div>
            </div>

            <div class="m26-channel-perf-grid">
              <div v-for="ch in analyticsData.channels" :key="ch.channel_name" class="m26-channel-perf-card">
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-2">
                    <span class="m26-channel-dot"></span>
                    <span class="font-semibold text-xs text-main">{{ ch.channel_name }}</span>
                  </div>
                  <span class="m26-badge emerald text-[11px]" v-if="ch.success_rate >= 95">{{ ch.success_rate.toFixed(0) }}% 成功</span>
                  <span class="m26-badge amber text-[11px]" v-else>{{ ch.success_rate.toFixed(0) }}% 成功</span>
                </div>

                <div class="grid grid-cols-3 gap-2 mt-3 pt-3 border-t border-slate-100">
                  <div>
                    <span class="text-[10.5px] text-muted block">承载请求</span>
                    <span class="font-semibold text-xs text-main font-mono">{{ ch.total_requests }} 次</span>
                  </div>
                  <div>
                    <span class="text-[10.5px] text-muted block">平均耗时</span>
                    <span class="font-semibold text-xs font-mono" :class="getLatencyTagClass(ch.avg_latency_ms)">
                      {{ formatAnalyticsLatency(ch.avg_latency_ms) }}
                    </span>
                  </div>
                  <div>
                    <span class="text-[10.5px] text-muted block">首字用时</span>
                    <span class="font-semibold text-xs text-main font-mono">
                      {{ formatAnalyticsLatency(ch.avg_ttft_ms) }}
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- SECTION 2: CHANNELS -->
        <section v-if="activeSection === 'channels'" class="m26-section-view">
          <!-- Filter & Controls Toolbar -->
          <div class="m26-control-bar">
            <!-- Left: Search, Sort By, Layout Mode -->
            <div class="m26-tools-left">
              <!-- Search Bar in Toolbar -->
              <div class="m26-toolbar-search">
                <svg class="m26-search-icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <circle cx="6.5" cy="6.5" r="4.5" />
                  <line x1="10" y1="10" x2="14" y2="14" />
                </svg>
                <input
                  ref="searchInputRef"
                  v-model="channelSearch"
                  type="text"
                  class="m26-toolbar-search-input"
                  placeholder="搜索渠道/URL/模型..."
                />
                <button v-if="channelSearch" class="m26-search-clear" @click="channelSearch = ''" title="清空搜索">✕</button>
                <span v-else class="m26-kbd">⌘K</span>
              </div>

              <!-- View Switch -->
              <div class="m26-segmented">
                <button
                  class="m26-seg-btn icon-only"
                  :class="{ active: channelViewMode === 'grid' }"
                  @click="setChannelViewMode('grid')"
                  title="网格卡片视图"
                >
                  <svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
                    <rect x="2" y="2" width="5" height="5" rx="1.2" />
                    <rect x="9" y="2" width="5" height="5" rx="1.2" />
                    <rect x="2" y="9" width="5" height="5" rx="1.2" />
                    <rect x="9" y="9" width="5" height="5" rx="1.2" />
                  </svg>
                </button>
                <button
                  class="m26-seg-btn icon-only"
                  :class="{ active: channelViewMode === 'list' }"
                  @click="setChannelViewMode('list')"
                  title="高密度表格视图"
                >
                  <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                    <line x1="2" y1="4" x2="14" y2="4" />
                    <line x1="2" y1="8" x2="14" y2="8" />
                    <line x1="2" y1="12" x2="14" y2="12" />
                  </svg>
                </button>
              </div>
            </div>

            <!-- Right: Actions -->
            <div class="m26-tools-right">
              <button class="m26-glass-btn" @click="probeAllChannels" title="探活全部渠道 (⌘⇧P)">
                <span>批量探活</span>
              </button>

              <!-- Add Channel Button -->
              <button class="m26-action-btn primary" @click="showAddChannel = true" title="添加渠道 (⌘N)">
                <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.2">
                  <line x1="8" y1="3" x2="8" y2="13" />
                  <line x1="3" y1="8" x2="13" y2="8" />
                </svg>
                <span>添加渠道</span>
              </button>
            </div>
          </div>

          <!-- Card Grid View -->
          <div v-if="channelViewMode === 'grid'" class="m26-channels-grid">
            <div
              v-for="(c, idx) in sortedChannels"
              :key="channelRenderKey(c, idx)"
              class="m26-hg-card"
              :class="[c.status, { disabled: !c.enabled, 'is-selected': isChannelSelected(c) }]"
              tabindex="0"
              @click="selectChannel(c)"
            >
              <!-- Card Header: Priority, Title, Toggle, Badges -->
              <div class="m26-card-header">
                <div class="m26-card-title-group">
                  <h4 class="m26-card-name" :title="c.name">{{ c.name }}</h4>
                </div>

                <div class="m26-card-badges-group">
                  <!-- Status Pill -->
                  <span
                    class="m26-card-status-pill"
                    :class="c.status"
                    :title="getStatusTooltip(c)"
                  >
                    {{ c.status === 'healthy' ? '就绪' : c.status === 'degraded' ? '高延迟' : '异常' }}
                  </span>

                  <!-- Enabled / Disabled Switch -->
                  <label class="m26-card-switch" :title="c.enabled ? '已启用（点击禁用）' : '已停用（点击启用）'">
                    <input type="checkbox" :checked="c.enabled" @change="toggleChannel(c)" />
                    <span class="slider"></span>
                  </label>
                </div>
              </div>

              <!-- Service / Role Tag Row -->
              <div class="m26-card-tag-row">
                <div class="m26-service-tag" :class="{ 'warning': c.circuitState && c.circuitState !== 'closed' }">
                  <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M6 3h4M6 13h4M8 3v10M3 8h10"/>
                  </svg>
                  <span v-if="c.circuitState && c.circuitState !== 'closed'">{{ circuitLabel(c.circuitState) }} · 自动熔断保护</span>
                  <span v-else>路由服务通道 · {{ getIntervalDisplay(c.probeInterval) }}探活</span>
                </div>
              </div>

              <!-- Models Section: 1 line per model, left model name, right latency / error -->
              <div class="m26-card-models-list">
                <div class="m26-models-header">
                  <span class="m26-models-title">支持模型 ({{ getChannelModels(c).length }})</span>
                </div>

                <div class="m26-model-rows">
                  <div
                    v-for="m in getChannelModels(c).slice(0, 3)"
                    :key="m"
                    class="m26-model-row"
                    :class="{ 'has-error': isModelFailed(c, m) }"
                  >
                    <!-- Left: Model Name -->
                    <div class="m26-model-row-name" :title="m">
                      <span class="m26-model-bullet" :class="{ error: isModelFailed(c, m) }"></span>
                      <span class="m26-model-name-text">{{ m }}</span>
                    </div>

                    <!-- Right: Model Latency or Error -->
                    <div class="m26-model-row-latency">
                      <!-- If error: clickable error button/badge that opens diagnostic modal -->
                      <button
                        v-if="isModelFailed(c, m)"
                        class="m26-model-err-badge"
                        @click.stop="openModelErrorDetail(c, m)"
                        :title="'模型异常：' + getModelErrorMessage(c, m) + ' (点击查看详细报错与诊断)'"
                      >
                        <span class="m26-err-dot"></span>
                        <span>{{ getModelLatencyDisplay(c, m) }}</span>
                        <span class="m26-err-arrow">➔</span>
                      </button>

                      <!-- If healthy / degraded / probing -->
                      <span
                        v-else
                        class="m26-model-lat-tag"
                        :class="getModelLatencyClass(c, m)"
                        :title="getModelTooltip(c, m)"
                        @click.stop="openModelErrorDetail(c, m)"
                      >
                        <span v-if="isModelProbing(c, m)" class="m26-spin-dot mini"></span>
                        <span>{{ getModelLatencyDisplay(c, m) }}</span>
                      </span>
                    </div>
                  </div>

                  <!-- If more than 3 models: collapse button that opens modal with all models -->
                  <button
                    v-if="getChannelModels(c).length > 3"
                    class="m26-model-more-btn"
                    @click.stop="openModelLatencyModal(c)"
                    :title="'查看全部 ' + getChannelModels(c).length + ' 个模型详情'"
                  >
                    <span>查看全部 {{ getChannelModels(c).length }} 个模型 (展开余下 {{ getChannelModels(c).length - 3 }} 个)</span>
                    <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                      <polyline points="5 3 10 8 5 13" />
                    </svg>
                  </button>
                </div>
              </div>

              <!-- 4 Dedicated Action Buttons: 探活, 编辑, 复制, 删除 -->
              <div class="m26-card-actions">
                <button
                  class="m26-card-btn"
                  :disabled="c.probing"
                  @click="probeChannel(c)"
                  title="立即对该渠道所有模型发起探活"
                >
                  <svg v-if="!c.probing" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M23 4v6h-6M1 20v-6h6"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>
                  </svg>
                  <span v-else class="m26-spin-dot"></span>
                  <span>{{ c.probing ? '探活中' : '探活' }}</span>
                </button>
                <button
                  class="m26-card-btn"
                  @click="openEditChannel(c)"
                  title="编辑渠道配置"
                >
                  <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/>
                  </svg>
                  <span>编辑</span>
                </button>
                <button
                  class="m26-card-btn"
                  @click="duplicateChannel(c)"
                  title="复制渠道"
                >
                  <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                  </svg>
                  <span>复制</span>
                </button>
                <button
                  class="m26-card-btn danger"
                  @click="removeChannel(c)"
                  title="删除此渠道"
                >
                  <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>
                  </svg>
                  <span>删除</span>
                </button>
              </div>
            </div>
          </div>

          <!-- High-Density Table View -->
          <div v-else class="m26-table-wrap">
            <table class="m26-table">
              <thead>
                <tr>
                  <th style="width: 160px;">渠道名称</th>
                  <th style="width: 120px;">接口协议</th>
                  <th style="width: 190px;">支持模型</th>
                  <th style="width: 170px;">健康状态</th>
                  <th style="width: 90px;">延迟</th>
                  <th style="width: 165px;">上次探活</th>
                  <th style="width: 65px; text-align: center;">启停</th>
                  <th style="width: 220px;" class="text-right">操作</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="(c, idx) in sortedChannels"
                  :key="channelRenderKey(c, idx)"
                  :class="{ 'is-selected': isChannelSelected(c) }"
                  @click="selectChannel(c)"
                >
                  <td>
                    <span class="font-medium text-main truncate block" :title="c.name">{{ c.name }}</span>
                  </td>
                  <td><span class="m26-proto-pill" :class="c.protocol">{{ c.protocol }}</span></td>
                  <td class="cursor-pointer" @click="openModelLatencyModal(c)" title="点击查看全部模型与测速">
                    <span class="m26-model-badge primary">
                      <span class="badge-icon">⚡</span>
                      {{ c.modelNames?.[0] || '默认' }}
                    </span>
                    <span v-if="(c.modelNames?.length || 0) > 1" class="m26-model-badge count ml-1">
                      +{{ (c.modelNames?.length || 1) - 1 }}
                    </span>
                  </td>
                  <td>
                    <div
                      class="m26-ch-status-box"
                      :class="{ 'has-error': c.lastError && c.status !== 'healthy' }"
                      :title="getStatusTooltip(c) + (c.lastError ? '（点击查看报错详情）' : '')"
                      @click="c.lastError && c.status !== 'healthy' ? openChannelDiagnostic(c) : null"
                    >
                      <span class="m26-status-dot" :class="c.status"></span>
                      <span>{{ statusLabel(c) }}</span>
                      <span v-if="c.lastError && c.status !== 'healthy'" class="m26-status-err-tag">查看原因</span>
                    </div>
                  </td>
                  <td class="font-mono whitespace-nowrap">{{ c.latency ? `${c.latency}ms` : '—' }}</td>
                  <td class="text-xs text-muted font-mono whitespace-nowrap">{{ formatLastCheckTime(c.lastCheck) }}</td>
                  <td style="text-align: center;">
                    <label class="m26-switch micro" :title="c.enabled ? '已启用（点击停用）' : '已停用（点击启用）'">
                      <input type="checkbox" :checked="c.enabled" @change="toggleChannel(c)" />
                      <span class="m26-slider"></span>
                    </label>
                  </td>
                  <td class="text-right">
                    <div class="m26-table-actions">
                      <button class="m26-btn-micro probe" :disabled="c.probing" @click="probeChannel(c)">探活</button>
                      <button
                        v-if="c.lastError && c.status !== 'healthy'"
                        class="m26-btn-micro danger-soft"
                        @click="openChannelDiagnostic(c)"
                        title="查看报错原因与排查建议"
                      >
                        诊断
                      </button>
                      <button class="m26-btn-icon" @click="duplicateChannel(c)" title="复制副本">
                        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                          <rect x="5" y="5" width="8" height="8" rx="1.5" />
                          <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                        </svg>
                      </button>
                      <button class="m26-btn-icon" @click="openEditChannel(c)" title="编辑"><svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor"><path d="M11 2l3 3-8 8H3v-3l8-8z"/></svg></button>
                      <button class="m26-btn-icon danger" @click="removeChannel(c)" title="删除"><svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor"><path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6"/></svg></button>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <!-- SECTION 3: MODELS -->
        <section v-if="activeSection === 'models'" class="m26-section-view">
          <div class="m26-models-page-header">
            <div class="m26-models-title-area">
              <div class="m26-family-pill-group">
                <button
                  v-for="tab in modelFamilyTabs"
                  :key="tab.key"
                  class="m26-family-pill-btn"
                  :class="{ active: selectedModelFamily === tab.key }"
                  @click="selectedModelFamily = tab.key"
                >
                  <span>{{ tab.label }}</span>
                  <span class="m26-family-count">{{ tab.count }}</span>
                </button>
              </div>
            </div>

            <div class="m26-models-actions-area">
              <div class="m26-toolbar-search">
                <svg class="m26-search-icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <circle cx="6.5" cy="6.5" r="4.5" />
                  <line x1="10" y1="10" x2="14" y2="14" />
                </svg>
                <input
                  v-model="channelSearch"
                  type="text"
                  class="m26-toolbar-search-input"
                  placeholder="搜索模型名称/渠道/别名..."
                />
                <button v-if="channelSearch" class="m26-search-clear" @click="channelSearch = ''" title="清空搜索">✕</button>
              </div>
              <button
                class="m26-action-btn secondary"
                @click="openCustomMappingsModal"
                title="自定义模型归类与别名合并设置"
              >
                <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M4 6h16M4 12h10M4 18h14"/>
                </svg>
                <span>模型归类设置</span>
                <span v-if="Object.keys(customModelMappings).length > 0" class="m26-badge-count">{{ Object.keys(customModelMappings).length }}</span>
              </button>
              <button
                v-if="channels.length"
                class="m26-action-btn primary"
                :disabled="isProbingAllModels"
                @click="probeAllActiveModels"
              >
                <span v-if="isProbingAllModels" class="m26-spin-dot"></span>
                <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>
                </svg>
                <span>{{ isProbingAllModels ? '测速中...' : '一键测速全部' }}</span>
              </button>
            </div>
          </div>

          <div class="m26-models-grid" v-if="filteredModels.length">
            <div v-for="m in filteredModels" :key="m.name" class="m26-model-card">
              <!-- Top: Brand Badge + Status/Latency Pill -->
              <div class="m26-model-card-top">
                <div
                  class="m26-model-brand-badge"
                  :style="{
                    color: getModelFamily(m.name).color,
                    background: getModelFamily(m.name).bg,
                    borderColor: getModelFamily(m.name).border
                  }"
                >
                  <span class="m26-model-brand-dot" :style="{ background: getModelFamily(m.name).color }"></span>
                  <span class="m26-model-brand-name">{{ getModelFamily(m.name).label }}</span>
                </div>

                <div class="m26-model-status-chip" :class="m.status">
                  <span class="m26-status-indicator-dot" :class="m.status"></span>
                  <span class="m26-status-indicator-text font-mono">
                    {{ m.status === 'healthy' ? m.latency : m.status === 'degraded' ? `${m.latency} 波动` : '离线' }}
                  </span>
                </div>
              </div>

              <!-- Main Info: Canonical Name + Copy Action -->
              <div class="m26-model-main-info">
                <div class="m26-model-title-box">
                  <span class="m26-model-canonical-name font-mono" :title="m.name">{{ m.name }}</span>
                  <button
                    class="m26-model-copy-btn"
                    @click.stop="copyModelId(m.name)"
                    :title="'复制模型名称: ' + m.name"
                  >
                    <svg v-if="copiedModelId !== m.name" viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8">
                      <rect x="5" y="5" width="8" height="8" rx="1.5" />
                      <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                    </svg>
                    <svg v-else viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="#059669" stroke-width="2.2">
                      <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                    </svg>
                    <span>{{ copiedModelId === m.name ? '已复制' : '复制' }}</span>
                  </button>
                </div>

                <!-- Aliases Row: Compact & Clickable -->
                <div
                  v-if="m.aliases && m.aliases.length > 0"
                  class="m26-model-aliases-bar clickable"
                  @click.stop="openAliasModal(m)"
                  :title="'点击查看全部 ' + m.aliases.length + ' 个上游别名与渠道路由映射'"
                >
                  <span class="m26-aliases-tag">
                    <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2">
                      <circle cx="18" cy="18" r="3"/>
                      <circle cx="6" cy="6" r="3"/>
                      <path d="M13 6h3a2 2 0 0 1 2 2v7"/>
                      <line x1="6" y1="9" x2="6" y2="21"/>
                    </svg>
                    <span>{{ m.aliases.length }} 个上游别名</span>
                  </span>
                  <span class="m26-aliases-text truncate font-mono">
                    {{ m.aliases.slice(0, 2).join(', ') }}{{ m.aliases.length > 2 ? ` 等 +${m.aliases.length - 2}` : '' }}
                  </span>
                  <span class="m26-aliases-more-btn">
                    <span>查看全部</span>
                    <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M6 12l4-4-4-4"/>
                    </svg>
                  </span>
                </div>
                <div
                  v-else
                  class="m26-model-aliases-bar direct clickable"
                  @click.stop="openAliasModal(m)"
                  title="点击查看各渠道同名直通映射明细"
                >
                  <span class="m26-aliases-tag direct">
                    <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2.5">
                      <polyline points="20 6 9 17 4 12"/>
                    </svg>
                    <span>标准同名直通</span>
                  </span>
                  <span class="m26-aliases-text">各渠道模型名一致</span>
                  <span class="m26-aliases-more-btn">
                    <span>明细</span>
                    <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M6 12l4-4-4-4"/>
                    </svg>
                  </span>
                </div>
              </div>

              <!-- Channel Pills Box -->
              <div class="m26-model-channels-box">
                <div class="m26-channels-meta-head">
                  <span class="m26-channels-count">提供渠道 ({{ m.providerCount }} 家)</span>
                </div>
                <div class="m26-channels-pill-list">
                  <span
                    v-for="ch in m.channelDetails.slice(0, 4)"
                    :key="ch.name"
                    class="m26-channel-mini-pill"
                    :class="ch.status"
                    :title="`${ch.name} · ${ch.status === 'healthy' ? '就绪' : ch.status === 'degraded' ? '延迟较高' : '离线'}${ch.latency ? ' · ' + ch.latency + 'ms' : ''}${ch.upstreamModel ? ' · 上游: ' + ch.upstreamModel : ''}`"
                  >
                    <span class="prov-dot" :class="ch.status"></span>
                    <span class="truncate max-w-[90px]">{{ ch.name }}</span>
                  </span>
                  <span
                    v-if="m.channelDetails.length > 4"
                    class="m26-channel-mini-pill more"
                    :title="'更多提供渠道：\n' + m.channelDetails.slice(4).map(c => c.name).join('\n')"
                  >
                    +{{ m.channelDetails.length - 4 }}
                  </span>
                </div>
              </div>

              <!-- Footer: Test & Classify Actions -->
              <div class="m26-model-card-footer">
                <div class="flex items-center gap-2">
                  <button
                    class="m26-model-test-btn"
                    @click="openModelSpeedTestFor(m.name)"
                    title="在全部支持此模型的渠道中发起并发测速"
                  >
                    <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
                      <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
                    </svg>
                    <span>测试此模型</span>
                  </button>
                  <button
                    class="m26-model-classify-btn"
                    @click.stop="openClassifyDialog(m.name)"
                    title="将此模型归入或合并到其他标准模型"
                  >
                    <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M2 4h12M4 8h8M6 12h4" />
                    </svg>
                    <span>归并</span>
                  </button>
                </div>
              </div>
            </div>
          </div>
          <div class="m26-empty-state" v-else>
            <p v-if="channelSearch || selectedModelFamily !== 'all'">未找到匹配的模型条件。</p>
            <p v-else>暂无可用模型。请先在渠道管理中添加并配置模型。</p>
          </div>
        </section>

        <!-- SECTION 4: LOGS -->
        <section v-if="activeSection === 'logs'" class="m26-section-view">
          <div class="m26-logs-header">
            <div class="m26-logs-left flex items-center gap-3">
              <span class="m26-logs-count">
                实时流水日志
                <span class="m26-badge ml-1">{{ filteredLogs.length }}</span>
                <span v-if="channelSearch" class="text-xs text-muted ml-2">(过滤自 {{ desktopLogs.length }} 条)</span>
              </span>
              <div class="m26-toolbar-search">
                <svg class="m26-search-icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <circle cx="6.5" cy="6.5" r="4.5" />
                  <line x1="10" y1="10" x2="14" y2="14" />
                </svg>
                <input
                  v-model="channelSearch"
                  type="text"
                  class="m26-toolbar-search-input"
                  placeholder="搜索流水日志..."
                />
                <button v-if="channelSearch" class="m26-search-clear" @click="channelSearch = ''" title="清空搜索">✕</button>
              </div>
            </div>
            <div class="m26-logs-right">
              <div
                class="m26-stream-status-pill"
                :class="{ connected: logsStreamConnected }"
                :title="logsStreamConnected ? '本地实时事件推流已就绪 (0ms 延迟)' : '本地实时流连接中...'"
              >
                <span class="m26-live-indicator"></span>
                <span>{{ logsStreamConnected ? '实时推流' : '连接中' }}</span>
              </div>
              <button
                class="m26-glass-btn"
                :class="{ 'm26-btn-active-blue': autoScrollLogs }"
                @click="autoScrollLogs = !autoScrollLogs"
                :title="autoScrollLogs ? '点击暂停自动滚动' : '点击开启新日志自动滚动到顶'"
              >
                <span v-if="autoScrollLogs">📜 实时滚动：开</span>
                <span v-else>⏸ 实时滚动：关</span>
              </button>
              <button class="m26-glass-btn danger" :disabled="isClearingLogs" @click="handleClearLogs">清空流水</button>
            </div>
          </div>

          <div class="m26-table-wrap" ref="logsTableWrapRef" v-if="filteredLogs.length">
            <table class="m26-table font-mono text-xs">
              <thead>
                <tr>
                  <th style="width: 165px;" class="whitespace-nowrap">请求时间</th>
                  <th style="width: 95px;" class="whitespace-nowrap">状态</th>
                  <th style="width: 155px;" class="whitespace-nowrap">模型</th>
                  <th style="width: 220px;" class="whitespace-nowrap">供应商 / 轮转</th>
                  <th style="width: 130px;" class="whitespace-nowrap">用时 / 首字</th>
                  <th style="width: 75px;" class="whitespace-nowrap">提示</th>
                  <th style="width: 75px;" class="whitespace-nowrap">补全</th>
                  <th style="width: 65px;" class="text-right whitespace-nowrap">详情</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="log in filteredLogs"
                  :key="log.id"
                  class="cursor-pointer transition-colors"
                  :class="[log.status_code === 0 ? 'm26-row-in-flight' : 'hover:bg-slate-50/80']"
                  @click="openLogDetail(log)"
                  title="点击查看请求与轮转详情"
                >
                  <td class="text-muted whitespace-nowrap">{{ formatLogDateTime(log.created_at) }}</td>
                  <td class="whitespace-nowrap">
                    <span
                      class="m26-status-pill"
                      :class="log.status_code === 0 ? 'status-streaming' : log.is_failover && log.status_code >= 200 && log.status_code < 300 ? 'status-failover' : log.status_code >= 200 && log.status_code < 300 ? 'status-200' : log.status_code === 429 ? 'status-429' : 'status-err'"
                    >
                      {{ log.status_code === 0 ? '传输中' : log.is_failover && log.status_code >= 200 && log.status_code < 300 ? '轮转 200' : log.status_code >= 200 && log.status_code < 300 ? '200 OK' : log.status_code === 429 ? '429 限流' : (log.status_code + ' 异常') }}
                    </span>
                  </td>
                  <td>
                    <div class="flex flex-col items-start w-fit">
                      <span class="m26-model-badge" :title="log.model">
                        <svg class="m26-model-badge-icon" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                          <circle cx="12" cy="12" r="9" />
                          <path d="M12 3a9 9 0 0 1 9 9 9 9 0 0 1-9 9 9 9 0 0 1-9-9 9 9 0 0 1 9-9z" />
                          <path d="M12 7v10M7 12h10" />
                        </svg>
                        <span class="truncate max-w-[145px] font-mono">{{ log.model }}</span>
                      </span>
                      <span v-if="log.upstream_model && log.upstream_model !== log.model" class="text-[10px] text-slate-400 font-mono pl-1 pt-0.5 truncate max-w-[145px]" :title="'映射至上游: ' + log.upstream_model">
                        ➔ {{ log.upstream_model }}
                      </span>
                    </div>
                  </td>
                  <td>
                    <div v-if="log.is_failover && log.failover_from" class="m26-failover-flow" :title="log.failover_from + ' ➔ ' + log.channel_name">
                      <template v-for="(node, nIdx) in getCompactFailoverNodes(log.failover_from)" :key="nIdx">
                        <span class="m26-tag-failover-from">{{ node }}</span>
                        <span class="m26-failover-arrow">➔</span>
                      </template>
                      <span class="m26-tag-failover-to">{{ log.channel_name }}</span>
                    </div>
                    <span v-else class="m26-channel-name-compact truncate block" :title="log.channel_name">{{ log.channel_name }}</span>
                  </td>
                  <td class="whitespace-nowrap">
                    <div class="flex items-center gap-1.5 flex-nowrap">
                      <span class="m26-pill-total" :title="'总响应耗时: ' + log.latency_ms + 'ms'">
                        {{ formatDurationPill(log.latency_ms) }}
                      </span>
                      <span class="m26-pill-ttft" :title="'首字耗时 (TTFT): ' + (log.ttft_ms || log.latency_ms) + 'ms'">
                        {{ formatDurationPill(log.ttft_ms || log.latency_ms) }}
                      </span>
                      <span v-if="log.is_stream" class="m26-pill-stream" title="SSE 流式传输">流</span>
                    </div>
                  </td>
                  <td class="text-slate-600 font-mono text-xs whitespace-nowrap">
                    {{ log.prompt_tokens ?? 0 }}
                  </td>
                  <td class="whitespace-nowrap">
                    <div class="flex items-center gap-1 font-mono text-xs text-slate-800 font-medium">
                      <span v-if="log.status_code === 0" class="m26-token-pulse-dot" title="流式生成中..."></span>
                      <span>{{ log.completion_tokens ?? 0 }}</span>
                    </div>
                  </td>
                  <td class="text-right whitespace-nowrap">
                    <span class="m26-link-btn text-xs">详情 ➔</span>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <div class="m26-empty-state" v-else>
            <p v-if="channelSearch">没有匹配“{{ channelSearch }}”的请求流水。</p>
            <p v-else>暂无网关请求流水。当第三方客户端（如 Cline、Chatbox、Cursor）向本地网关发起请求时将实时记录。</p>
          </div>
        </section>

        <!-- SECTION 5: SETTINGS -->
        <section v-if="activeSection === 'settings'" class="m26-section-view">
          <div class="m26-settings-container">
            <!-- Group 1: Routing Engine -->
            <div class="m26-pref-group">
              <div class="m26-pref-group-header">
                <div class="m26-pref-group-icon icon-route">
                  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="18" cy="18" r="3"/>
                    <circle cx="6" cy="6" r="3"/>
                    <path d="M13 6h3a2 2 0 0 1 2 2v7"/>
                    <line x1="6" y1="9" x2="6" y2="21"/>
                  </svg>
                </div>
                <div class="m26-pref-group-titles">
                  <h3 class="m26-pref-title">分发与路由引擎</h3>
                  <span class="m26-pref-subtitle">配置网关收到客户端请求时的多通道调度算法与熔断顺延机制</span>
                </div>
              </div>

              <div class="m26-route-cards">
                <!-- Card 1: Smart Quality -->
                <div
                  class="m26-route-card"
                  :class="{ selected: routingStrategy === 'smart_quality' || routingStrategy === 'priority' }"
                  @click="setRoutingStrategy('smart_quality')"
                >
                  <div class="m26-route-card-head">
                    <div class="m26-route-type-badge smart_quality">
                      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
                        <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/>
                      </svg>
                    </div>
                    <div class="m26-route-check-indicator">
                      <svg v-if="routingStrategy === 'smart_quality' || routingStrategy === 'priority'" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3">
                        <polyline points="20 6 9 17 4 12"/>
                      </svg>
                    </div>
                  </div>
                  <div class="m26-route-body">
                    <div class="m26-route-name-wrap">
                      <span class="m26-route-name">智能综合质量</span>
                      <span class="m26-route-code-tag">Smart Quality</span>
                    </div>
                    <p class="m26-route-desc">综合就绪状态、实时延迟与历史稳定性动态评分，遇 429 限流或失败自动降权避让。</p>
                  </div>
                </div>

                <!-- Card 2: Lowest Latency -->
                <div
                  class="m26-route-card"
                  :class="{ selected: routingStrategy === 'latency' }"
                  @click="setRoutingStrategy('latency')"
                >
                  <div class="m26-route-card-head">
                    <div class="m26-route-type-badge latency">
                      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
                        <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>
                      </svg>
                    </div>
                    <div class="flex items-center gap-2">
                      <span class="m26-route-pill-rec">推荐</span>
                      <div class="m26-route-check-indicator">
                        <svg v-if="routingStrategy === 'latency'" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3">
                          <polyline points="20 6 9 17 4 12"/>
                        </svg>
                      </div>
                    </div>
                  </div>
                  <div class="m26-route-body">
                    <div class="m26-route-name-wrap">
                      <span class="m26-route-name">最低延迟优先</span>
                      <span class="m26-route-code-tag">Lowest Latency</span>
                    </div>
                    <p class="m26-route-desc">实时选取探活延迟最低的可用通道发起调用，提供极致响应速度与流畅体感。</p>
                  </div>
                </div>

                <!-- Card 3: Round Robin -->
                <div
                  class="m26-route-card"
                  :class="{ selected: routingStrategy === 'round_robin' }"
                  @click="setRoutingStrategy('round_robin')"
                >
                  <div class="m26-route-card-head">
                    <div class="m26-route-type-badge roundrobin">
                      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67"/>
                      </svg>
                    </div>
                    <div class="m26-route-check-indicator">
                      <svg v-if="routingStrategy === 'round_robin'" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3">
                        <polyline points="20 6 9 17 4 12"/>
                      </svg>
                    </div>
                  </div>
                  <div class="m26-route-body">
                    <div class="m26-route-name-wrap">
                      <span class="m26-route-name">轮询均衡</span>
                      <span class="m26-route-code-tag">Round Robin</span>
                    </div>
                    <p class="m26-route-desc">在所有正常状态的通道中均匀分配流量，防止单个账号频繁调用限流。</p>
                  </div>
                </div>
              </div>
            </div>

            <!-- Group 2: Gateway Auth & Server -->
            <div class="m26-pref-group">
              <div class="m26-pref-group-header">
                <div class="m26-pref-group-icon icon-auth">
                  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                    <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
                    <path d="M7 11V7a5 5 0 0 1 10 0v4"/>
                  </svg>
                </div>
                <div class="m26-pref-group-titles">
                  <h3 class="m26-pref-title">网关接入与本地密钥</h3>
                  <span class="m26-pref-subtitle">本机分发服务的统一监听地址与第三方客户端身份校验凭证</span>
                </div>
              </div>

              <div class="m26-pref-card">
                <div class="m26-pref-row">
                  <div class="m26-pref-meta">
                    <span class="label">网关 Base URL</span>
                    <span class="desc">填入开发工具（如 Claude Code / Cursor / Codex CLI / Cline）中的 API 基础端点</span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <div class="m26-cred-field">
                      <span class="m26-cred-field-tag">ENDPOINT</span>
                      <span class="m26-cred-field-val font-mono" @click="copyGatewayUrl" title="点击复制 Base URL">{{ gatewayBaseUrl }}</span>
                      <button class="m26-cred-field-btn" @click="copyGatewayUrl" title="复制 Base URL">
                        <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                          <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                        </svg>
                        <span>复制</span>
                      </button>
                    </div>
                  </div>
                </div>

                <div class="m26-pref-row">
                  <div class="m26-pref-meta">
                    <span class="label">网关安全密钥 (API Key)</span>
                    <span class="desc">用于验证本地调用者身份（客户端发起请求时通过 Bearer Token 传输）</span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <div class="m26-cred-field">
                      <span class="m26-cred-field-tag">SECRET</span>
                      <span class="m26-cred-field-val font-mono" @click="copyApiKey" title="点击复制 API Key">
                        {{ showApiKey ? (gatewayApiKey || '暂无 Key') : (maskedOverviewApiKey || '••••••••') }}
                      </span>
                      <div class="m26-cred-field-actions">
                        <button
                          class="m26-cred-icon-btn"
                          @click="showApiKey = !showApiKey"
                          :title="showApiKey ? '隐藏 API Key' : '显示完整 API Key'"
                        >
                          <svg v-if="!showApiKey" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                            <circle cx="12" cy="12" r="3"/>
                          </svg>
                          <svg v-else viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                            <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
                            <line x1="1" y1="1" x2="23" y2="23"/>
                          </svg>
                        </button>
                        <button class="m26-cred-field-btn" @click="copyApiKey" title="复制 API Key">
                          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                            <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                          </svg>
                          <span>复制</span>
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>

            <!-- Group 3: Storage & Migration -->
            <div class="m26-pref-group">
              <div class="m26-pref-group-header">
                <div class="m26-pref-group-icon icon-storage">
                  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                    <ellipse cx="12" cy="5" rx="9" ry="3"/>
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/>
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>
                  </svg>
                </div>
                <div class="m26-pref-group-titles">
                  <h3 class="m26-pref-title">数据存储与自动清理策略</h3>
                  <span class="m26-pref-subtitle">管理本地 SQLite 数据库文件、历史请求流水保留周期及配置备份</span>
                </div>
              </div>

              <div class="m26-pref-card">
                <div class="m26-pref-row" v-if="systemInfo?.db_path">
                  <div class="m26-pref-meta">
                    <div class="flex items-center gap-2">
                      <span class="label">SQLite 本地数据库</span>
                      <span class="m26-db-chip">{{ formatBytes(systemInfo.db_size_bytes) }}</span>
                      <span class="m26-db-chip">{{ (systemInfo.logs_count ?? desktopLogs.length).toLocaleString() }} 条流水</span>
                    </div>
                    <span class="desc font-mono text-xs opacity-75 truncate max-w-lg" :title="systemInfo.db_path">
                      {{ systemInfo.db_path }}
                    </span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <button class="m26-glass-btn" :disabled="isPruningLogs" @click="handlePruneLogs" title="按当前策略立即执行一次修剪清理">
                      <span v-if="isPruningLogs" class="m26-spin-dot"></span>
                      <svg v-else viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                        <polyline points="23 4 23 10 17 10"/>
                        <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
                      </svg>
                      <span>{{ isPruningLogs ? '清理中...' : '立即执行清理' }}</span>
                    </button>
                    <button class="m26-glass-btn" @click="copySystemPath(systemInfo.db_path, '数据库')" title="复制数据库绝对路径">
                      <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                        <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                        <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                      </svg>
                      <span>复制路径</span>
                    </button>
                  </div>
                </div>

                <div class="m26-pref-row">
                  <div class="m26-pref-meta">
                    <span class="label">自动清理保留时长</span>
                    <span class="desc">后台每 30 分钟定时自动巡检，物理删除超过此保留天数的历史请求与 Token 流水</span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <select
                      :value="logRetentionDays"
                      class="m26-select"
                      @change="handleSetLogRetention(Number(($event.target as HTMLSelectElement).value))"
                    >
                      <option :value="1">保留 1 天 (极轻量)</option>
                      <option :value="3">保留 3 天 (日常轻量)</option>
                      <option :value="7">保留 7 天 (默认推荐)</option>
                      <option :value="14">保留 14 天</option>
                      <option :value="30">保留 30 天</option>
                      <option :value="0">永久保留 (不限时长)</option>
                    </select>
                  </div>
                </div>

                <div class="m26-pref-row">
                  <div class="m26-pref-meta">
                    <span class="label">自动清理条数上限</span>
                    <span class="desc">流水总记录数超出上限阈值时，后台静默按 FIFO（先进先出）修剪删除最早记录</span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <select
                      :value="logMaxCount"
                      class="m26-select"
                      @change="handleSetLogMaxCount(Number(($event.target as HTMLSelectElement).value))"
                    >
                      <option :value="1000">最多 1,000 条 (约 0.5 MB)</option>
                      <option :value="3000">最多 3,000 条 (约 1.5 MB)</option>
                      <option :value="5000">最多 5,000 条 (默认推荐 · 约 2.5 MB)</option>
                      <option :value="10000">最多 10,000 条 (约 5 MB)</option>
                      <option :value="50000">最多 50,000 条</option>
                      <option :value="0">不限制条数</option>
                    </select>
                  </div>
                </div>

                <div class="m26-pref-row">
                  <div class="m26-pref-meta">
                    <span class="label">配置备份与工作区重置</span>
                    <span class="desc">以 JSON 格式完整导出本机通道与偏好设置供备份迁移，或一键重置当前工作区</span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <div class="flex items-center gap-2">
                      <button class="m26-glass-btn" @click="exportConfig" title="导出当前所有渠道与配置">
                        <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
                          <polyline points="7 10 12 15 17 10"/>
                          <line x1="12" y1="15" x2="12" y2="3"/>
                        </svg>
                        <span>导出配置</span>
                      </button>
                      <button class="m26-glass-btn" @click="openImport" title="导入已有配置文件">
                        <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
                          <polyline points="17 8 12 3 7 8"/>
                          <line x1="12" y1="3" x2="12" y2="15"/>
                        </svg>
                        <span>导入配置</span>
                      </button>
                      <div class="m26-btn-separator"></div>
                      <button class="m26-glass-btn danger" @click="clearWorkspace" title="清空本机保存的所有通道配置">
                        <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
                          <polyline points="3 6 5 6 21 6"/>
                          <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>
                        </svg>
                        <span>清空工作区</span>
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            </div>

            <!-- Group 4: Custom Model Classification & Alias Merging -->
            <div class="m26-pref-group">
              <div class="m26-pref-group-header">
                <div class="m26-pref-group-icon icon-storage">
                  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M4 6h16M4 12h10M4 18h14"/>
                  </svg>
                </div>
                <div class="m26-pref-group-titles">
                  <h3 class="m26-pref-title">自定义模型归类与别名合并</h3>
                  <span class="m26-pref-subtitle">手动将上游特殊或非规范命名的模型合并到标准模型中，统一聚合卡片与多渠道容灾调度</span>
                </div>
              </div>

              <div class="m26-pref-card">
                <!-- Add New Mapping Rule -->
                <div class="m26-pref-row">
                  <div class="m26-pref-meta">
                    <span class="label">新建归类映射规则</span>
                    <span class="desc">将渠道上报的原始模型（如 deepseek-v4-flash-0731-free-3）强制映射至目标聚合模型</span>
                  </div>
                  <div class="m26-pref-ctrl">
                    <div class="flex items-center gap-2 flex-wrap">
                      <div class="flex items-center gap-1.5">
                        <span class="text-xs text-muted">原始模型:</span>
                        <input
                          v-model="newMappingSource"
                          type="text"
                          class="m26-input font-mono !h-[30px] !w-[220px] !text-xs"
                          placeholder="例如: deepseek-v4-flash-0731-free-3"
                          list="source-model-suggestions"
                        />
                        <datalist id="source-model-suggestions">
                          <option v-for="m in allDetectedChannelModels" :key="m" :value="m" />
                        </datalist>
                      </div>
                      <span class="text-xs text-muted">➔ 归入:</span>
                      <div class="flex items-center gap-1.5">
                        <input
                          v-model="newMappingTarget"
                          type="text"
                          class="m26-input font-mono !h-[30px] !w-[180px] !text-xs"
                          placeholder="例如: deepseek-v4-flash"
                          list="target-model-suggestions"
                        />
                        <datalist id="target-model-suggestions">
                          <option v-for="m in existingCanonicalModelNames" :key="m" :value="m" />
                        </datalist>
                      </div>
                      <button
                        class="m26-action-btn primary !h-[30px] !text-xs"
                        :disabled="!newMappingSource.trim() || !newMappingTarget.trim()"
                        @click="handleAddSettingsMapping"
                      >
                        <span>添加归类</span>
                      </button>
                    </div>
                  </div>
                </div>

                <!-- Existing Rules List -->
                <div class="m26-pref-row !block" v-if="Object.keys(customModelMappings).length > 0">
                  <div class="w-full">
                    <div class="m26-custom-mappings-table">
                      <div class="m26-mapping-row header">
                        <span class="col-source">原始上游模型名</span>
                        <span class="col-arrow"></span>
                        <span class="col-target">归入标准模型</span>
                        <span class="col-action">操作</span>
                      </div>
                      <div
                        v-for="(target, raw) in customModelMappings"
                        :key="raw"
                        class="m26-mapping-row"
                      >
                        <span class="col-source font-mono">{{ raw }}</span>
                        <span class="col-arrow">➔</span>
                        <span class="col-target font-mono font-semibold text-indigo-600">{{ target }}</span>
                        <span class="col-action">
                          <button
                            class="m26-btn-icon danger"
                            @click="handleRemoveCustomMapping(String(raw))"
                            title="删除此条归类映射规则"
                          >
                            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor">
                              <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6"/>
                            </svg>
                          </button>
                        </span>
                      </div>
                    </div>
                  </div>
                </div>
                <div class="m26-pref-row" v-else>
                  <div class="text-xs text-slate-400 py-2">
                    暂无自定义归类规则。系统当前按内置智能规约算法聚合。遇到未识别的特殊名称可在上方添加映射，或在【模型】卡片上直接点击“归并”。
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>
      </div>
    </main>

    <!-- Hidden Input for Import -->
    <input ref="configInput" type="file" accept=".json" class="hidden" @change="importConfig" />

    <!-- MODAL: ADD CHANNEL -->
    <div v-if="showAddChannel" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showAddChannel = false)">
      <div class="m26-modal-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <h3 class="m26-modal-title">添加供应商渠道</h3>
          <button class="m26-close-btn" data-no-drag @click="showAddChannel = false">✕</button>
        </div>

        <div class="m26-modal-body">
          <div class="m26-form-group">
            <label class="m26-label">渠道名称</label>
            <input v-model="newChannelName" type="text" class="m26-input" placeholder="例如：Anthropic 官方主力 / DeepSeek V3" />
          </div>

          <div class="m26-form-row">
            <div class="m26-form-group">
              <label class="m26-label">接口协议</label>
              <select v-model="newChannelProtocol" class="m26-select">
                <option value="openai-compatible">OpenAI Compatible (Chat Completions)</option>
                <option value="anthropic">Anthropic Claude (/v1/messages)</option>
                <option value="openai-responses">OpenAI Responses API (原生补全)</option>
              </select>
            </div>
            <div class="m26-form-group">
              <label class="m26-label">探活频率</label>
              <select v-model="newChannelProbePreset" class="m26-select">
                <option v-for="opt in intervalPresets" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
              </select>
              <div v-if="newChannelProbePreset === -1" class="m26-custom-interval-wrap mt-1.5">
                <div class="flex items-center gap-2">
                  <input
                    v-model.number="newChannelProbeCustom"
                    type="number"
                    min="15"
                    max="86400"
                    class="m26-input text-xs"
                    placeholder="间隔秒数 (≥15)"
                  />
                  <span class="text-xs text-slate-500 shrink-0">秒/次</span>
                </div>
              </div>
            </div>
          </div>

          <div class="m26-form-group">
            <label class="m26-label">Base URL (API 端点)</label>
            <input v-model="newChannelUrl" type="text" class="m26-input" placeholder="https://api.openai.com/v1" />
          </div>

          <div class="m26-form-group">
            <label class="m26-label">API 密钥 (API Key)</label>
            <div class="m26-input-wrapper">
              <input
                v-model="newChannelKey"
                :type="showNewKey ? 'text' : 'password'"
                class="m26-input"
                placeholder="sk-..."
              />
              <button
                type="button"
                class="m26-input-eye"
                tabindex="-1"
                @click="showNewKey = !showNewKey"
                :title="showNewKey ? '隐藏密钥' : '显示密钥'"
              >
                <svg v-if="showNewKey" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
                  <line x1="1" y1="1" x2="23" y2="23" />
                </svg>
                <svg v-else viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                  <circle cx="12" cy="12" r="3" />
                </svg>
              </button>
            </div>
          </div>

          <div class="m26-form-group">
            <div class="flex justify-between items-center mb-1">
              <label class="m26-label mb-0">支持模型列表 (首行为探活主模型)</label>
              <button class="m26-text-btn" :disabled="isFetchingNewModels" @click="fetchUpstreamModels('new')">
                {{ isFetchingNewModels ? '拉取中...' : '自动拉取上游模型' }}
              </button>
            </div>
            <textarea v-model="newChannelModelText" rows="3" class="m26-textarea font-mono text-xs" placeholder="gpt-4o-mini&#10;gpt-4o&#10;claude-3-7-sonnet"></textarea>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="showAddChannel = false">取消</button>
          <button class="m26-action-btn primary" @click="addChannel">确认创建</button>
        </div>
      </div>
    </div>

    <!-- MODAL: EDIT CHANNEL -->
    <div v-if="showEditChannel" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showEditChannel = false)">
      <div class="m26-modal-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <h3 class="m26-modal-title">编辑渠道配置</h3>
          <button class="m26-close-btn" data-no-drag @click="showEditChannel = false">✕</button>
        </div>

        <div class="m26-modal-body">
          <div class="m26-form-group">
            <label class="m26-label">渠道名称</label>
            <input v-model="editChannelName" type="text" class="m26-input" />
          </div>

          <div class="m26-form-row">
            <div class="m26-form-group">
              <label class="m26-label">接口协议</label>
              <select v-model="editChannelProtocol" class="m26-select">
                <option value="openai-compatible">OpenAI Compatible (Chat Completions)</option>
                <option value="anthropic">Anthropic Claude (/v1/messages)</option>
                <option value="openai-responses">OpenAI Responses API (原生补全)</option>
              </select>
            </div>
            <div class="m26-form-group">
              <label class="m26-label">探活频率</label>
              <select v-model="editChannelProbePreset" class="m26-select">
                <option v-for="opt in intervalPresets" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
              </select>
              <div v-if="editChannelProbePreset === -1" class="m26-custom-interval-wrap mt-1.5">
                <div class="flex items-center gap-2">
                  <input
                    v-model.number="editChannelProbeCustom"
                    type="number"
                    min="15"
                    max="86400"
                    class="m26-input text-xs"
                    placeholder="间隔秒数 (≥15)"
                  />
                  <span class="text-xs text-slate-500 shrink-0">秒/次</span>
                </div>
              </div>
            </div>
          </div>

          <div class="m26-form-group">
            <label class="m26-label">Base URL (API 端点)</label>
            <input v-model="editChannelUrl" type="text" class="m26-input" />
          </div>

          <div class="m26-form-group">
            <label class="m26-label">API 密钥</label>
            <div class="m26-input-wrapper">
              <input
                v-model="editChannelKey"
                :type="showEditKey ? 'text' : 'password'"
                class="m26-input"
                placeholder="请输入 API 密钥"
              />
              <button
                type="button"
                class="m26-input-eye"
                tabindex="-1"
                @click="showEditKey = !showEditKey"
                :title="showEditKey ? '隐藏密钥' : '显示密钥'"
              >
                <svg v-if="showEditKey" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
                  <line x1="1" y1="1" x2="23" y2="23" />
                </svg>
                <svg v-else viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                  <circle cx="12" cy="12" r="3" />
                </svg>
              </button>
            </div>
          </div>

          <div class="m26-form-group">
            <div class="flex justify-between items-center mb-1">
              <label class="m26-label mb-0">支持模型列表 (首行为探活主模型)</label>
              <button class="m26-text-btn" :disabled="isFetchingEditModels" @click="fetchUpstreamModels('edit')">
                {{ isFetchingEditModels ? '拉取中...' : '自动拉取上游模型' }}
              </button>
            </div>
            <textarea v-model="editChannelModelText" rows="3" class="m26-textarea font-mono text-xs"></textarea>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="showEditChannel = false">取消</button>
          <button class="m26-action-btn primary" @click="saveEditChannel">保存修改</button>
        </div>
      </div>
    </div>

    <!-- MODAL: CONFIRM ACTION -->
    <div v-if="confirmModal" class="m26-modal-backdrop m26-confirm-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => confirmModal = null)">
      <div class="m26-modal-sheet m26-confirm-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <h3 class="m26-modal-title">{{ confirmModal.title }}</h3>
          <button class="m26-close-btn" data-no-drag @click="confirmModal = null">✕</button>
        </div>
        <div class="m26-modal-body">
          <div class="m26-confirm-content">
            <div class="m26-confirm-icon-wrap" :class="{ danger: confirmModal.danger }">
              <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 9v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
            </div>
            <div class="m26-confirm-texts">
              <p class="m26-confirm-main">{{ confirmModal.message }}</p>
              <p v-if="confirmModal.description" class="m26-confirm-desc">{{ confirmModal.description }}</p>
            </div>
          </div>
        </div>
        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="confirmModal = null">取消</button>
          <button
            class="m26-action-btn"
            :class="confirmModal.danger ? 'danger' : 'primary'"
            @click="handleConfirmModalAction"
          >
            {{ confirmModal.confirmText || '确认' }}
          </button>
        </div>
      </div>
    </div>

    <!-- MODAL: MODEL LATENCY & SPEED TEST -->
    <div v-if="activeModelLatencyChannel" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => activeModelLatencyChannel = null)">
      <div class="m26-modal-sheet m26-models-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div>
            <div class="flex items-center gap-2">
              <h3 class="m26-modal-title">全部模型与实时测速</h3>
              <span class="m26-proto-pill" :class="activeModelLatencyChannel.protocol">
                {{ activeModelLatencyChannel.protocol }}
              </span>
            </div>
            <p class="m26-modal-subtitle">
              渠道 “{{ activeModelLatencyChannel.name }}” · 共 {{ activeModelLatencyChannel.modelNames?.length || 1 }} 个模型
            </p>
          </div>
          <button class="m26-close-btn" data-no-drag @click="activeModelLatencyChannel = null">✕</button>
        </div>

        <div class="m26-modal-toolbar">
          <div class="m26-modal-search">
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="6.5" cy="6.5" r="4.5" />
              <line x1="10" y1="10" x2="14" y2="14" />
            </svg>
            <input
              v-model="modelSearchQuery"
              type="text"
              placeholder="过滤模型名称..."
              class="m26-modal-search-input"
            />
          </div>

          <div class="flex items-center gap-2">
            <button
              v-if="getFailedModelsInChannel(activeModelLatencyChannel).length > 0"
              class="m26-action-btn danger-subtle"
              @click="cleanFailedModels(activeModelLatencyChannel)"
              title="一键移除该渠道中所有测速报错或异常的模型"
            >
              <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6" />
              </svg>
              <span>清理失效 ({{ getFailedModelsInChannel(activeModelLatencyChannel).length }})</span>
            </button>

            <button
              class="m26-action-btn primary"
              :disabled="isProbingAllModels"
              @click="probeAllChannelModels(activeModelLatencyChannel)"
            >
              <svg v-if="!isProbingAllModels" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
              </svg>
              <span v-else class="m26-spin-dot"></span>
              <span>{{ isProbingAllModels ? '全模型测速中...' : '一键测速全部模型' }}</span>
            </button>
          </div>
        </div>

        <div class="m26-modal-body m26-model-list-body">
          <div class="m26-model-grid">
            <div
              v-for="(modelName, idx) in filteredModalModels"
              :key="modelName"
              class="m26-model-item-card"
            >
              <div class="m26-model-item-info">
                <div class="flex items-center gap-2">
                  <span class="m26-model-idx">#{{ idx + 1 }}</span>
                  <span class="m26-model-full-name" :title="modelName">{{ modelName }}</span>
                  <span v-if="isPrimaryModel(activeModelLatencyChannel, modelName)" class="m26-tag-primary">探活主模型</span>
                </div>
                <span class="m26-model-ch-url">{{ activeModelLatencyChannel.url }}</span>
              </div>

              <div class="m26-model-item-actions">
                <!-- Latency Badge -->
                <div
                  class="m26-model-lat-pill"
                  :class="[getModelLatencyClass(activeModelLatencyChannel, modelName), { 'has-err-link': isModelFailed(activeModelLatencyChannel, modelName) }]"
                  :title="getModelTooltip(activeModelLatencyChannel, modelName)"
                  @click="openModelErrorDetail(activeModelLatencyChannel, modelName)"
                >
                  <span class="lat-dot"></span>
                  <span class="lat-text">{{ getModelLatencyDisplay(activeModelLatencyChannel, modelName) }}</span>
                  <span v-if="isModelFailed(activeModelLatencyChannel, modelName)" class="lat-view-err">报错 ➔</span>
                </div>

                <!-- Test button -->
                <button
                  class="m26-btn-micro probe"
                  :disabled="isModelProbing(activeModelLatencyChannel, modelName)"
                  @click="probeSingleModel(activeModelLatencyChannel, modelName)"
                  title="单独测试此模型真实可用性与时延"
                >
                  <span v-if="isModelProbing(activeModelLatencyChannel, modelName)" class="m26-spin-dot"></span>
                  <span>{{ isModelProbing(activeModelLatencyChannel, modelName) ? '测试中' : '测速' }}</span>
                </button>

                <!-- Copy button -->
                <button
                  class="m26-btn-icon"
                  @click="copyModelName(modelName)"
                  title="复制模型 ID"
                >
                  <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                    <rect x="5" y="5" width="8" height="8" rx="1.5" />
                    <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                  </svg>
                </button>

                <!-- Delete Model button -->
                <button
                  class="m26-btn-icon danger"
                  @click.stop="removeModelFromChannel(activeModelLatencyChannel, modelName)"
                  title="从当前渠道删除此模型"
                >
                  <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                    <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6" />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>

        <div class="m26-modal-footer">
          <span class="m26-modal-hint text-xs text-slate-400">
            💡 测速说明：HeiGate 会向模型发送真实轻量测试请求。不仅测试网络端到端往返耗时，同时严格验证密钥鉴权、账户额度与模型实际生成可用性。
          </span>
        </div>
      </div>
    </div>

    <!-- MODAL: MODEL MULTI-PROVIDER SPEED TEST (测试此模型所有供应商) -->
    <div
      v-if="activeModelSpeedTestName"
      class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, closeModelSpeedTestModal)"
    >
      <div class="m26-modal-sheet m26-speedtest-modal-sheet" @click.stop>
        <!-- Header -->
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-3">
            <div
              class="m26-model-brand-badge shrink-0"
              :style="{
                color: getModelFamily(activeModelSpeedTestName).color,
                background: getModelFamily(activeModelSpeedTestName).bg,
                borderColor: getModelFamily(activeModelSpeedTestName).border,
              }"
            >
              <span class="m26-model-brand-dot" :style="{ background: getModelFamily(activeModelSpeedTestName).color }"></span>
              <span class="m26-model-brand-name whitespace-nowrap">{{ getModelFamily(activeModelSpeedTestName).label }}</span>
            </div>
            <div>
              <div class="flex items-center gap-2">
                <h3 class="m26-modal-title font-mono">{{ activeModelSpeedTestName }}</h3>
                <span class="m26-modal-count-pill">
                  共 {{ activeModelProviders.length }} 家供应商
                </span>
                <button
                  class="m26-model-copy-btn"
                  @click.stop="copyModelId(activeModelSpeedTestName)"
                  :title="'复制模型名: ' + activeModelSpeedTestName"
                >
                  <svg v-if="copiedModelId !== activeModelSpeedTestName" viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8">
                    <rect x="5" y="5" width="8" height="8" rx="1.5" />
                    <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                  </svg>
                  <svg v-else viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="#059669" stroke-width="2.2">
                    <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                  </svg>
                  <span>{{ copiedModelId === activeModelSpeedTestName ? '已复制' : '复制' }}</span>
                </button>
              </div>
              <p class="m26-modal-subtitle">
                同一模型多供应商并发测速 · 检验该模型在各渠道的端到端真实生成响应与延迟梯度
              </p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="closeModelSpeedTestModal">✕</button>
        </div>

        <!-- Toolbar -->
        <div class="m26-modal-toolbar">
          <div class="m26-modal-search">
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="6.5" cy="6.5" r="4.5" />
              <line x1="10" y1="10" x2="14" y2="14" />
            </svg>
            <input
              v-model="modelSpeedTestQuery"
              type="text"
              placeholder="搜索供应商名称、上游别名或接口地址..."
              class="m26-modal-search-input"
            />
          </div>

          <div class="flex items-center gap-2">
            <div v-if="getFastestProviderInfo(activeModelSpeedTestName)" class="m26-fastest-provider-pill">
              <span class="m26-fastest-dot"></span>
              <span>⚡ 最优: {{ getFastestProviderInfo(activeModelSpeedTestName)!.channelName }} ({{ getFastestProviderInfo(activeModelSpeedTestName)!.latency }}ms)</span>
            </div>

            <button
              class="m26-action-btn primary"
              :disabled="isProbingModelProviders"
              @click="probeAllProvidersForModel(activeModelSpeedTestName)"
              title="重新对该模型的所有供应商发起并发真实测速"
            >
              <svg v-if="!isProbingModelProviders" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
              </svg>
              <span v-else class="m26-spin-dot"></span>
              <span>{{ isProbingModelProviders ? '全渠道并发测速中...' : '重新测速全部供应商' }}</span>
            </button>
          </div>
        </div>

        <!-- Body: Providers List -->
        <div class="m26-modal-body m26-speedtest-body">
          <div v-if="activeModelProviders.length > 0" class="m26-model-grid">
            <div
              v-for="(p, idx) in activeModelProviders"
              :key="p.channel.desktopId || p.channel.name"
              class="m26-model-item-card"
              :class="{ 'is-preferred': isFastestProvider(p) }"
            >
              <div class="m26-model-item-info">
                <div class="flex items-center gap-2 flex-wrap">
                  <span class="m26-model-idx">#{{ idx + 1 }}</span>
                  <span class="m26-channel-name font-semibold">{{ p.channel.name }}</span>
                  <span class="m26-proto-pill" :class="p.channel.protocol">{{ p.channel.protocol }}</span>

                  <span v-if="isFastestProvider(p)" class="m26-tag-preferred">
                    ⚡ 优选首选 (网关优先调度)
                  </span>
                  <span v-if="!p.channel.enabled" class="m26-tag-disabled">已暂停</span>
                </div>

                <div class="flex items-center gap-2 mt-1 text-xs text-slate-500 dark:text-slate-400 flex-wrap">
                  <span class="m26-speedtest-alias-tag">
                    上游别名: <code class="font-mono text-indigo-600 dark:text-indigo-400">{{ p.upstreamModel }}</code>
                  </span>
                  <span class="m26-model-ch-url" :title="p.channel.url">{{ p.channel.url }}</span>
                </div>
              </div>

              <div class="m26-model-item-actions">
                <!-- Latency Badge -->
                <div
                  class="m26-model-lat-pill"
                  :class="[p.latencyClass, { 'has-err-link': p.isFailed }]"
                  :title="p.tooltip"
                  @click="p.isFailed ? openModelErrorDetail(p.channel, p.upstreamModel) : null"
                >
                  <span class="lat-dot"></span>
                  <span class="lat-text">{{ p.latencyDisplay }}</span>
                  <span v-if="p.isFailed" class="lat-view-err">报错 ➔</span>
                </div>

                <!-- Test single provider button -->
                <button
                  class="m26-btn-micro probe"
                  :disabled="p.isProbing"
                  @click="probeSingleModel(p.channel, p.upstreamModel)"
                  title="单独重新测速此供应商"
                >
                  <span v-if="p.isProbing" class="m26-spin-dot"></span>
                  <span>{{ p.isProbing ? '测试中' : '测速' }}</span>
                </button>

                <!-- Diagnostic details button -->
                <button
                  class="m26-btn-icon"
                  @click="openModelErrorDetail(p.channel, p.upstreamModel)"
                  title="查看此渠道完整诊断信息"
                >
                  <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                    <circle cx="8" cy="8" r="6"/>
                    <path d="M8 7v4M8 5h.01"/>
                  </svg>
                </button>
              </div>
            </div>
          </div>
          <div v-else class="m26-empty-state py-8 text-center text-slate-400">
            <p v-if="modelSpeedTestQuery">未找到匹配 “{{ modelSpeedTestQuery }}” 的供应商渠道</p>
            <p v-else>暂无支持该模型的供应商渠道</p>
          </div>
        </div>

        <!-- Footer -->
        <div class="m26-modal-footer">
          <span class="m26-modal-hint text-xs text-slate-400">
            💡 网关调度规则：本地请求该模型时，HeiGate 将首选上方延迟最低的健康供应商（带 ⚡ 优选首选 标识）。若单次调用遇到异常或限流，将执行 1 次就地快速重试，仍失败则自动无缝轮转至次优供应商，全方位保障高可用。
          </span>
        </div>
      </div>
    </div>

    <!-- MODAL: MODEL ALIASES DETAIL -->
    <div
      v-if="activeAliasModalModel"
      class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, closeAliasModal)"
    >
      <div class="m26-modal-sheet m26-aliases-modal-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-3">
            <span
              class="m26-model-family-badge"
              :style="{
                color: getModelFamily(activeAliasModalModel.name).color,
                background: getModelFamily(activeAliasModalModel.name).bg,
                borderColor: getModelFamily(activeAliasModalModel.name).border,
              }"
            >
              <span class="m26-badge-dot" :style="{ background: getModelFamily(activeAliasModalModel.name).color }"></span>
              <span>{{ getModelFamily(activeAliasModalModel.name).label }}</span>
            </span>
            <div>
              <div class="flex items-center gap-2">
                <h3 class="m26-modal-title font-mono">{{ activeAliasModalModel.name }}</h3>
                <button
                  class="m26-model-copy-btn"
                  @click.stop="copyModelId(activeAliasModalModel.name)"
                  :title="'复制模型名: ' + activeAliasModalModel.name"
                >
                  <svg v-if="copiedModelId !== activeAliasModalModel.name" viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8">
                    <rect x="5" y="5" width="8" height="8" rx="1.5" />
                    <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                  </svg>
                  <svg v-else viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="#059669" stroke-width="2.2">
                    <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                  </svg>
                  <span>{{ copiedModelId === activeAliasModalModel.name ? '已复制' : '复制' }}</span>
                </button>
              </div>
              <p class="m26-modal-subtitle">
                统一网关模型 · 聚合 {{ activeAliasModalModel.aliases.length }} 个上游别名 · 共 {{ activeAliasModalModel.providerCount }} 个供货渠道
              </p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="closeAliasModal">✕</button>
        </div>

        <div class="m26-modal-toolbar">
          <div class="m26-modal-search">
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="6.5" cy="6.5" r="4.5" />
              <line x1="10" y1="10" x2="14" y2="14" />
            </svg>
            <input
              v-model="aliasModalSearch"
              type="text"
              placeholder="搜索别名或渠道名称..."
              class="m26-modal-search-input"
            />
          </div>
          <div class="flex items-center gap-2">
            <button
              class="m26-action-btn secondary"
              @click="openModelSpeedTestFor(activeAliasModalModel.name)"
              title="并发测速此模型全部供应商的可用性与延迟"
            >
              <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
                <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
              </svg>
              <span>测速全部供应商</span>
            </button>
            <button
              class="m26-action-btn secondary"
              @click="openClassifyDialog('', activeAliasModalModel.name)"
              title="将其他渠道模型手动归并到此聚合模型"
            >
              <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M8 3v10M3 8h10"/>
              </svg>
              <span>归入新别名</span>
            </button>
            <button
              v-if="activeAliasModalModel.aliases.length > 0"
              class="m26-action-btn secondary"
              @click="copyAllAliases(activeAliasModalModel)"
              title="复制全部上游别名（每行一个）"
            >
              <svg v-if="copiedAliasText !== '__ALL__'" viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
                <rect x="5" y="5" width="8" height="8" rx="1.5" />
                <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
              </svg>
              <svg v-else viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="#059669" stroke-width="2.2">
                <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
              </svg>
              <span>{{ copiedAliasText === '__ALL__' ? '已复制全部' : '复制全部别名' }}</span>
            </button>
          </div>
        </div>

        <div class="m26-modal-body m26-alias-modal-body">
          <!-- Guide -->
          <div class="m26-alias-guide-card">
            <div class="m26-alias-guide-header">
              <div class="flex items-center gap-2">
                <span class="m26-guide-badge">外部调用规范</span>
                <span class="text-xs text-slate-500">外部应用调用此聚合模型时请求参数填写：</span>
              </div>
            </div>
            <div class="m26-alias-guide-code font-mono">
              <span class="text-indigo-600 font-semibold">"model"</span>: <span class="text-emerald-700">"{{ activeAliasModalModel.name }}"</span>
            </div>
          </div>

          <!-- Aliases Breakdown Section -->
          <div class="m26-alias-section">
            <div class="m26-alias-section-head">
              <span class="m26-alias-section-title">
                聚合的上游原始别名 ({{ filteredModalAliases.length }})
              </span>
              <span class="text-xs text-slate-400">各渠道实际声明的上游模型标识</span>
            </div>

            <div v-if="filteredModalAliases.length === 0" class="m26-empty-alias-tip">
              <span v-if="activeAliasModalModel.aliases.length === 0">
                该模型无额外上游别名，所有提供渠道均直接使用标准名 <code>{{ activeAliasModalModel.name }}</code>。
              </span>
              <span v-else>未找到符合过滤条件的上游别名。</span>
            </div>

            <div v-else class="m26-alias-items-grid">
              <div
                v-for="aliasItem in filteredModalAliases"
                :key="aliasItem.alias"
                class="m26-alias-item-card"
              >
                <div class="m26-alias-card-top">
                  <div class="flex items-center gap-2">
                    <div class="m26-alias-name font-mono" :title="aliasItem.alias">
                      {{ aliasItem.alias }}
                    </div>
                    <span
                      v-if="customModelMappings[aliasItem.alias] || customModelMappings[aliasItem.alias.toLowerCase()]"
                      class="m26-custom-mapping-badge"
                      title="由用户手动添加的归类映射规则"
                    >
                      手动归类
                    </span>
                  </div>
                  <div class="flex items-center gap-1">
                    <button
                      class="m26-alias-copy-btn"
                      @click.stop="copyAlias(aliasItem.alias)"
                      :title="'复制别名: ' + aliasItem.alias"
                    >
                      <svg v-if="copiedAliasText !== aliasItem.alias" viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.8">
                        <rect x="5" y="5" width="8" height="8" rx="1.5" />
                        <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                      </svg>
                      <svg v-else viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="#059669" stroke-width="2.2">
                        <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                      </svg>
                      <span>{{ copiedAliasText === aliasItem.alias ? '已复制' : '复制' }}</span>
                    </button>
                    <button
                      v-if="customModelMappings[aliasItem.alias] || customModelMappings[aliasItem.alias.toLowerCase()]"
                      class="m26-alias-unlink-btn"
                      @click.stop="handleRemoveCustomMapping(aliasItem.alias)"
                      title="解除手动归类，恢复为独立模型"
                    >
                      解除
                    </button>
                  </div>
                </div>

                <div class="m26-alias-providers-row">
                  <span class="m26-alias-prov-label">对应供货渠道：</span>
                  <div class="m26-alias-prov-chips">
                    <span
                      v-for="ch in aliasItem.channels"
                      :key="ch.name"
                      class="m26-alias-prov-chip"
                      :class="ch.status"
                    >
                      <span class="prov-dot" :class="ch.status"></span>
                      <span>{{ ch.name }}</span>
                      <span v-if="ch.latency" class="ch-lat-num">{{ ch.latency }}ms</span>
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- Channel Dispatch Mapping Table -->
          <div class="m26-alias-section">
            <div class="m26-alias-section-head">
              <span class="m26-alias-section-title">
                各渠道调用与重写映射 ({{ filteredModalChannelDetails.length }})
              </span>
              <span class="text-xs text-slate-400">HeiGate 路由到具体渠道时的实时模型转发与重写策略</span>
            </div>

            <div class="m26-mapping-table-wrapper">
              <table class="m26-mapping-table">
                <thead>
                  <tr>
                    <th>渠道名称</th>
                    <th>实际转发上游模型</th>
                    <th>重写策略</th>
                    <th>状态与延迟</th>
                  </tr>
                </thead>
                <tbody>
                  <tr
                    v-for="ch in filteredModalChannelDetails"
                    :key="ch.name"
                  >
                    <td>
                      <div class="flex items-center gap-1.5 font-medium text-slate-800">
                        <span class="prov-dot" :class="ch.status"></span>
                        <span>{{ ch.name }}</span>
                      </div>
                    </td>
                    <td>
                      <div class="flex items-center gap-1.5 font-mono text-xs text-indigo-700 font-semibold">
                        <span>{{ ch.upstreamModel }}</span>
                        <button
                          class="m26-table-copy-icon"
                          @click.stop="copyAlias(ch.upstreamModel)"
                          :title="'复制上游模型名: ' + ch.upstreamModel"
                        >
                          <svg v-if="copiedAliasText !== ch.upstreamModel" viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.8">
                            <rect x="5" y="5" width="8" height="8" rx="1.5" />
                            <path d="M3 11V3a1.5 1.5 0 0 1 1.5-1.5h8" />
                          </svg>
                          <svg v-else viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="#059669" stroke-width="2.2">
                            <polyline points="13.5 4.5 6.5 11.5 2.5 7.5" />
                          </svg>
                        </button>
                      </div>
                    </td>
                    <td>
                      <span
                        v-if="ch.upstreamModel !== activeAliasModalModel.name"
                        class="m26-mapping-chip rewrite"
                        title="HeiGate 会自动重写模型名为该渠道期望的别名"
                      >
                        ↳ 别名重写
                      </span>
                      <span
                        v-else
                        class="m26-mapping-chip direct"
                        title="无需别名映射，直接以同名模型请求"
                      >
                        ✓ 同名直通
                      </span>
                    </td>
                    <td>
                      <div class="flex items-center gap-1.5 text-xs text-slate-600">
                        <span class="m26-status-text" :class="ch.status">
                          {{ ch.status === 'healthy' ? '正常' : ch.status === 'degraded' ? '延迟较高' : '离线' }}
                        </span>
                        <span v-if="ch.latency" class="font-mono text-slate-400">({{ ch.latency }}ms)</span>
                      </div>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <div class="m26-modal-footer">
          <div class="m26-modal-tip text-xs text-slate-400">
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
              <circle cx="8" cy="8" r="7" />
              <line x1="8" y1="7" x2="8" y2="11" />
              <circle cx="8" cy="4.5" r="0.7" fill="currentColor" />
            </svg>
            <span>HeiGate 会在分发调度时自动将统一模型名映射为上游别名，实现多渠道平滑无感负载均衡与故障熔断顺延。</span>
          </div>
          <button class="m26-action-btn" @click="closeAliasModal">关闭</button>
        </div>
      </div>
    </div>

    <!-- MODAL: QUICK MODEL CLASSIFY -->
    <div
      v-if="showClassifyModal"
      class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, () => showClassifyModal = false)"
    >
      <div class="m26-modal-sheet" style="max-width: 460px;">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M4 6h16M4 12h10M4 18h14"/>
            </svg>
            <h3 class="m26-modal-title">手动模型归并</h3>
          </div>
          <button class="m26-close-btn" data-no-drag @click="showClassifyModal = false">✕</button>
        </div>

        <div class="m26-modal-body">
          <p class="text-xs text-slate-500 mb-3">
            将指定的原始模型（或特殊命名别名）归并到标准模型中。归入后，该渠道将与目标模型在同一卡片中聚合，并享受同一模型的最低延迟调度与多渠道故障轮转。
          </p>

          <div class="m26-form-group">
            <label class="m26-label">待归并原始模型</label>
            <input
              v-model="classifySourceModel"
              type="text"
              class="m26-input font-mono"
              placeholder="例如: deepseek-v4-flash-0731-free-3"
              list="classify-source-suggestions"
            />
            <datalist id="classify-source-suggestions">
              <option v-for="m in allDetectedChannelModels" :key="m" :value="m" />
            </datalist>
          </div>

          <div class="m26-form-group">
            <label class="m26-label">归入目标聚合模型</label>
            <select v-model="classifyTargetModel" class="m26-select font-mono mb-2">
              <option
                v-for="name in existingCanonicalModelNames"
                :key="name"
                :value="name"
              >
                {{ name }}
              </option>
              <option value="__custom__">+ 输入新目标模型名称...</option>
            </select>
            <input
              v-if="classifyTargetModel === '__custom__'"
              v-model="classifyCustomTarget"
              type="text"
              class="m26-input font-mono"
              placeholder="输入目标模型规范名称，例如 deepseek-v4-flash"
            />
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="showClassifyModal = false">取消</button>
          <button class="m26-action-btn primary" @click="submitClassify">确认归并</button>
        </div>
      </div>
    </div>

    <!-- MODAL: CUSTOM MODEL MAPPINGS MANAGER -->
    <div
      v-if="showCustomMappingsModal"
      class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, () => showCustomMappingsModal = false)"
    >
      <div class="m26-modal-sheet" style="max-width: 580px;">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M4 6h16M4 12h10M4 18h14"/>
            </svg>
            <h3 class="m26-modal-title">自定义模型归类设置</h3>
          </div>
          <button class="m26-close-btn" data-no-drag @click="showCustomMappingsModal = false">✕</button>
        </div>

        <div class="m26-modal-body">
          <p class="text-xs text-slate-500 mb-3">
            在此管理手动归类映射。将上游非标准或私有模型名称重定向至标准模型，解决算法无法自动合并的问题。
          </p>

          <!-- Add Form -->
          <div class="m26-pref-card p-3 mb-3 bg-slate-50">
            <span class="text-xs font-semibold text-slate-700 block mb-2">添加新归类映射</span>
            <div class="flex items-center gap-2 flex-wrap">
              <input
                v-model="managerSourceModel"
                type="text"
                class="m26-input font-mono !h-[30px] !w-[210px] !text-xs"
                placeholder="原始模型 (如 deepseek-v4-flash-0731-free-3)"
                list="manager-source-suggestions"
              />
              <datalist id="manager-source-suggestions">
                <option v-for="m in allDetectedChannelModels" :key="m" :value="m" />
              </datalist>
              <span class="text-xs text-slate-400">➔</span>
              <select v-model="managerTargetModel" class="m26-select font-mono !h-[30px] !w-[160px] !text-xs">
                <option
                  v-for="name in existingCanonicalModelNames"
                  :key="name"
                  :value="name"
                >
                  {{ name }}
                </option>
                <option value="__custom__">+ 新目标模型...</option>
              </select>
              <input
                v-if="managerTargetModel === '__custom__'"
                v-model="managerCustomTarget"
                type="text"
                class="m26-input font-mono !h-[30px] !w-[150px] !text-xs"
                placeholder="目标模型名称"
              />
              <button
                class="m26-action-btn primary !h-[30px] !text-xs"
                :disabled="!managerSourceModel.trim()"
                @click="handleAddManagerMapping"
              >
                添加
              </button>
            </div>
          </div>

          <!-- Existing Mappings List -->
          <div v-if="Object.keys(customModelMappings).length > 0">
            <span class="text-xs font-semibold text-slate-700 block mb-1.5">
              已生效规则 ({{ Object.keys(customModelMappings).length }})
            </span>
            <div class="m26-custom-mappings-table">
              <div class="m26-mapping-row header">
                <span class="col-source">原始上游模型名</span>
                <span class="col-arrow"></span>
                <span class="col-target">归入标准模型</span>
                <span class="col-action">操作</span>
              </div>
              <div
                v-for="(target, raw) in customModelMappings"
                :key="raw"
                class="m26-mapping-row"
              >
                <span class="col-source font-mono">{{ raw }}</span>
                <span class="col-arrow">➔</span>
                <span class="col-target font-mono font-semibold text-indigo-600">{{ target }}</span>
                <span class="col-action">
                  <button
                    class="m26-btn-icon danger"
                    @click="handleRemoveCustomMapping(String(raw))"
                    title="解除此归类映射"
                  >
                    <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor">
                      <path d="M3 4h10M6 4V2h4v2M5 6v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V6"/>
                    </svg>
                  </button>
                </span>
              </div>
            </div>
          </div>
          <div v-else class="text-center py-6 text-xs text-slate-400">
            暂无自定义归类规则。在上方添加后，卡片将即时合并。
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-action-btn primary" @click="showCustomMappingsModal = false">完成</button>
        </div>
      </div>
    </div>

    <!-- MODAL: CHANNEL PROBE & ERROR DIAGNOSIS -->
    <div v-if="activeDiagnosticChannel" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => activeDiagnosticChannel = null)">
      <div class="m26-modal-sheet m26-diagnostic-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <div class="m26-diag-icon-badge danger">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 9v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
            </div>
            <div>
              <h3 class="m26-modal-title">探活诊断与报错详情</h3>
              <p class="m26-modal-subtitle">
                渠道 “{{ activeDiagnosticChannel.name }}” · {{ activeDiagnosticChannel.url }}
              </p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="activeDiagnosticChannel = null">✕</button>
        </div>

        <div class="m26-modal-body m26-diagnostic-body">
          <!-- 1. Diagnostic Summary Card -->
          <div class="m26-diag-card danger">
            <div class="m26-diag-card-title">
              <span class="m26-diag-dot danger"></span>
              <span>故障摘要</span>
              <span class="m26-diag-code-pill">{{ extractErrorCode(activeDiagnosticChannel.lastError) }}</span>
            </div>
            <div class="m26-diag-main-msg">
              {{ formatErrorSummary(activeDiagnosticChannel.lastError) }}
            </div>
          </div>

          <!-- 2. Request Details Grid -->
          <div class="m26-diag-meta-grid">
            <div class="m26-diag-meta-item">
              <span class="label">探活主模型</span>
              <span class="value font-mono">{{ activeDiagnosticChannel.modelNames?.[0] || '默认模型' }}</span>
            </div>
            <div class="m26-diag-meta-item">
              <span class="label">接口协议</span>
              <span class="value font-mono">{{ activeDiagnosticChannel.protocol }}</span>
            </div>
            <div class="m26-diag-meta-item">
              <span class="label">上游返回耗时</span>
              <span class="value font-mono">{{ activeDiagnosticChannel.latency ? `${activeDiagnosticChannel.latency}ms` : '超时' }}</span>
            </div>
            <div class="m26-diag-meta-item">
              <span class="label">最近检测时间</span>
              <span class="value">{{ activeDiagnosticChannel.lastCheck }}</span>
            </div>
          </div>

          <!-- 3. Smart Advice List -->
          <div class="m26-diag-card advice">
            <div class="m26-diag-card-title">
              <span class="m26-diag-dot info"></span>
              <span>智能排查与修复建议</span>
            </div>
            <div class="m26-diag-advice-list">
              <div v-for="(tip, i) in getTroubleshootingAdvice(activeDiagnosticChannel)" :key="i" class="m26-diag-tip-item">
                <span class="tip-num">{{ i + 1 }}</span>
                <span class="tip-txt" v-html="tip"></span>
              </div>
            </div>
          </div>

          <!-- 4. Raw Upstream Response Code Section -->
          <div class="m26-diag-raw-section">
            <div class="m26-diag-raw-header">
              <span class="label">上游原始回包数据 (Raw Response)</span>
              <button class="m26-text-btn" @click="copyDiagnosticRaw">复制完整报错</button>
            </div>
            <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ activeDiagnosticChannel.lastError || '无原始回包' }}</code></pre>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="openEditChannelFromDiag(activeDiagnosticChannel)">
            修改渠道配置
          </button>
          <button
            class="m26-action-btn primary"
            :disabled="activeDiagnosticChannel.probing"
            @click="retryProbeFromDiag(activeDiagnosticChannel)"
          >
            <span v-if="activeDiagnosticChannel.probing" class="m26-spin-dot"></span>
            <span>{{ activeDiagnosticChannel.probing ? '探活检测中...' : '重新检测' }}</span>
          </button>
          <button class="m26-glass-btn" @click="activeDiagnosticChannel = null">关闭</button>
        </div>
      </div>
    </div>

    <!-- MODAL: LOG DETAIL -->
    <div v-if="activeLogDetail" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => activeLogDetail = null)">
      <div class="m26-modal-sheet m26-log-detail-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <div
              class="m26-diag-icon-badge"
              :class="getLogDiagnostic(activeLogDetail).badge"
            >
              <svg v-if="getLogDiagnostic(activeLogDetail).badge === 'success'" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="20 6 9 17 4 12" />
              </svg>
              <svg v-else-if="getLogDiagnostic(activeLogDetail).badge === 'amber'" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="23 4 23 10 17 10" />
                <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
              </svg>
              <svg v-else-if="getLogDiagnostic(activeLogDetail).badge === 'info'" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="12" cy="12" r="10" />
                <polyline points="12 6 12 12 16 14" />
              </svg>
              <svg v-else viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 9v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
            </div>
            <div>
              <h3 class="m26-modal-title">请求流水详情 #{{ activeLogDetail.id }}</h3>
              <p class="m26-modal-subtitle">
                {{ formatLogTime(activeLogDetail.created_at) }} · 网关全链路诊断
              </p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="activeLogDetail = null">✕</button>
        </div>

        <div class="m26-modal-body m26-diagnostic-body">
          <!-- 1. Status Banner -->
          <div
            class="m26-diag-card"
            :class="getLogDiagnostic(activeLogDetail).badge"
          >
            <div class="m26-diag-card-title">
              <span
                class="m26-diag-dot"
                :class="getLogDiagnostic(activeLogDetail).badge"
              ></span>
              <span>{{ getLogDiagnostic(activeLogDetail).title }}</span>
              <span
                class="m26-diag-code-pill"
                :class="'code-' + getLogDiagnostic(activeLogDetail).badge"
              >
                {{ getLogDiagnostic(activeLogDetail).codeText }}
              </span>
            </div>
            <div class="m26-diag-main-msg">
              {{ getLogDiagnostic(activeLogDetail).message }}
            </div>
          </div>

          <!-- 2. Execution Trace Timeline (调用链路追踪) -->
          <div class="m26-log-section">
            <div class="m26-section-subtitle-bar">
              <span class="m26-subtitle-tag">链路追踪</span>
              <span class="m26-subtitle-title">多级重试与故障轮转链路 (Execution Trace)</span>
            </div>
            <div class="m26-trace-timeline">
              <div
                v-for="step in parseTraceSteps(activeLogDetail)"
                :key="step.index"
                class="m26-trace-item"
                :class="{ 'trace-item-success': step.succeeded, 'trace-item-fail': !step.succeeded }"
              >
                <div class="m26-trace-node">
                  <span class="m26-trace-index">{{ step.index }}</span>
                  <div class="m26-trace-line"></div>
                </div>
                <div class="m26-trace-card">
                  <div class="m26-trace-header">
                    <div class="flex items-center gap-2">
                      <span class="m26-trace-channel-name">{{ step.channel_name || '未知渠道' }}</span>
                      <span v-if="step.channel_id" class="m26-trace-id-badge">ID: {{ step.channel_id }}</span>
                      <span v-if="step.attempt > 1" class="m26-trace-retry-badge">重试 #{{ step.attempt }}</span>
                    </div>
                    <div class="flex items-center gap-1.5">
                      <span
                        class="m26-trace-code-pill"
                        :class="step.status_code >= 200 && step.status_code < 300 ? 'status-200' : step.status_code === 429 ? 'status-429' : 'status-err'"
                      >
                        {{ step.status_code ? `HTTP ${step.status_code}` : '超时/中断' }}
                      </span>
                      <span v-if="step.latency_ms > 0" class="m26-trace-latency">{{ step.latency_ms }}ms</span>
                    </div>
                  </div>
                  <div class="m26-trace-body">
                    <div class="m26-trace-spec-row">
                      <span class="label">端点:</span>
                      <span class="val font-mono truncate" :title="step.endpoint">{{ step.endpoint || '默认网关路由' }}</span>
                    </div>
                    <div class="m26-trace-spec-row">
                      <span class="label">模型:</span>
                      <span class="val font-mono">{{ step.model }}</span>
                      <span v-if="step.ttft_ms" class="val-ttft">TTFT: {{ step.ttft_ms }}ms</span>
                    </div>
                    <div v-if="step.error || step.raw_error" class="m26-trace-error-box">
                      <span class="error-icon">⚠️</span>
                      <span class="font-mono text-xs">{{ step.error || step.raw_error }}</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- 3. Performance & Tokens Summary (吞吐性能) -->
          <div class="m26-log-section">
            <div class="m26-section-subtitle-bar">
              <span class="m26-subtitle-tag">吞吐性能</span>
              <span class="m26-subtitle-title">Token 用量与响应延迟</span>
            </div>
            <div class="m26-perf-grid">
              <div class="m26-perf-card">
                <span class="label">首字耗时 (TTFT)</span>
                <span class="val font-mono">{{ activeLogDetail.ttft_ms || activeLogDetail.latency_ms }} <span class="unit">ms</span></span>
              </div>
              <div class="m26-perf-card">
                <span class="label">全程总耗时</span>
                <span class="val font-mono">{{ activeLogDetail.latency_ms }} <span class="unit">ms</span></span>
              </div>
              <div class="m26-perf-card">
                <span class="label">生成速率</span>
                <span class="val font-mono">{{ calculateTokenSpeed(activeLogDetail) }}</span>
              </div>
              <div class="m26-perf-card">
                <span class="label">提示词 Token</span>
                <span class="val font-mono">{{ activeLogDetail.prompt_tokens ?? 0 }}</span>
              </div>
              <div class="m26-perf-card">
                <span class="label">补全 Token</span>
                <span class="val font-mono text-emerald-600">{{ activeLogDetail.completion_tokens ?? 0 }}</span>
              </div>
              <div class="m26-perf-card">
                <span class="label">总 Token</span>
                <span class="val font-mono">{{ activeLogDetail.total_tokens ?? ((activeLogDetail.prompt_tokens || 0) + (activeLogDetail.completion_tokens || 0)) }}</span>
              </div>
            </div>
          </div>

          <!-- 4. Environment & Request Specs (网络与网关环境) -->
          <div class="m26-log-section">
            <div class="m26-section-subtitle-bar">
              <span class="m26-subtitle-tag">调用环境</span>
              <span class="m26-subtitle-title">客户端与网关路由参数</span>
            </div>
            <div class="m26-diag-meta-grid">
              <div class="m26-diag-meta-item">
                <span class="label">客户端地址 (Client IP)</span>
                <span class="value font-mono">{{ activeLogDetail.client_ip || '127.0.0.1 (本地)' }}</span>
              </div>
              <div class="m26-diag-meta-item">
                <span class="label">请求接口 (Endpoint Path)</span>
                <span class="value font-mono">{{ activeLogDetail.request_path || '/v1/chat/completions' }}</span>
              </div>
              <div class="m26-diag-meta-item">
                <span class="label">请求模型 (Requested)</span>
                <span class="value font-mono">{{ activeLogDetail.model || '通用模型' }}</span>
              </div>
              <div class="m26-diag-meta-item">
                <span class="label">上游映射模型 (Upstream)</span>
                <span class="value font-mono">{{ activeLogDetail.upstream_model || activeLogDetail.model || '通用模型' }}</span>
              </div>
              <div class="m26-diag-meta-item">
                <span class="label">传输类型</span>
                <span class="value">{{ activeLogDetail.is_stream ? 'SSE 流式传输 (Server-Sent Events)' : '同步阻塞传输 (Blocking)' }}</span>
              </div>
              <div class="m26-diag-meta-item">
                <span class="label">结束状态 (Finish Reason)</span>
                <span class="value font-mono">{{ activeLogDetail.finish_reason || (activeLogDetail.status_code === 200 ? 'stop' : 'unknown') }}</span>
              </div>
              <div class="m26-diag-meta-item col-span-2">
                <span class="label">最终承载渠道</span>
                <span class="value">{{ activeLogDetail.channel_name || '默认' }} (ID: {{ activeLogDetail.channel_id }}) · {{ activeLogDetail.endpoint || '默认地址' }}</span>
              </div>
              <div v-if="activeLogDetail.user_agent" class="m26-diag-meta-item col-span-2">
                <span class="label">客户端软件 (User-Agent)</span>
                <span class="value font-mono text-xs" :title="activeLogDetail.user_agent">{{ activeLogDetail.user_agent }}</span>
              </div>
            </div>
          </div>

          <!-- 5. Error message if any -->
          <div v-if="activeLogDetail.error_message" class="m26-diag-raw-section">
            <div class="m26-diag-raw-header">
              <span class="label">上游异常报错回包 (Error Diagnostics)</span>
              <button class="m26-text-btn" @click="copyErrorMessage(activeLogDetail.error_message)">复制报错</button>
            </div>
            <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ activeLogDetail.error_message }}</code></pre>
          </div>

          <!-- 6. Full JSON View -->
          <div class="m26-diag-raw-section">
            <div class="m26-diag-raw-header">
              <span class="label">完整流水数据 (JSON)</span>
              <button class="m26-text-btn" @click="copyLogJson(activeLogDetail)">复制完整 JSON</button>
            </div>
            <pre class="m26-diag-raw-code font-mono text-xs"><code>{{ JSON.stringify(activeLogDetail, null, 2) }}</code></pre>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="copyLogJson(activeLogDetail)">复制 JSON</button>
          <button class="m26-action-btn primary" @click="activeLogDetail = null">关闭</button>
        </div>
      </div>
    </div>

    <!-- MODAL: KEYBOARD SHORTCUTS CHEAT SHEET -->
    <div v-if="showShortcutsModal" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showShortcutsModal = false)">
      <div class="m26-modal-sheet m26-shortcuts-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <div class="m26-shortcuts-icon">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="2" y="4" width="20" height="16" rx="2.5" />
                <path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M6 12h.01M10 12h.01M14 12h.01M18 12h.01M7 16h10" />
              </svg>
            </div>
            <div>
              <h3 class="m26-modal-title">键盘快捷键速查</h3>
              <p class="m26-modal-subtitle">原生 macOS 全局键位与极速交互体验</p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="showShortcutsModal = false">✕</button>
        </div>

        <div class="m26-modal-body m26-shortcuts-body">
          <!-- Category 1: 全局导航 -->
          <div class="m26-shortcut-group">
            <div class="m26-shortcut-group-title">全局与视图切换</div>
            <div class="m26-shortcut-list">
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">切换页面 (总览 / 供应商 / 模型 / 日志 / 设置)</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">1 ~ 5</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">偏好设置</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">,</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">聚焦搜索框 / 全选关键字</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">K</span>
                  <span class="m26-kbd-or">或</span>
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">F</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">刷新全部数据与健康探活状态</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">R</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">打开 / 关闭此快捷键速查表</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">/</span>
                  <span class="m26-kbd-or">或</span>
                  <span class="m26-kbd-badge">?</span>
                </div>
              </div>
            </div>
          </div>

          <!-- Category 2: 供应商与渠道运维 -->
          <div class="m26-shortcut-group">
            <div class="m26-shortcut-group-title">供应商渠道与探活操作</div>
            <div class="m26-shortcut-list">
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">新建供应商渠道</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">N</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">批量探活全部渠道所有模型</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">⇧</span>
                  <span class="m26-kbd-badge">P</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">编辑当前选中渠道</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">E</span>
                  <span class="m26-kbd-or">或</span>
                  <span class="m26-kbd-badge">E</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">键盘方向键切换选定卡片</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">←</span>
                  <span class="m26-kbd-badge">→</span>
                  <span class="m26-kbd-badge">↑</span>
                  <span class="m26-kbd-badge">↓</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">单路通道快速探活</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">P</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">启用 / 停用当前选中的渠道</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">Space</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">删除选中的渠道</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">Delete</span>
                </div>
              </div>
            </div>
          </div>

          <!-- Category 3: 弹窗与通用剪贴板 -->
          <div class="m26-shortcut-group">
            <div class="m26-shortcut-group-title">弹窗与文本交互</div>
            <div class="m26-shortcut-list">
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">关闭弹窗 / 退出聚焦</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">Esc</span>
                  <span class="m26-kbd-or">或</span>
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">W</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">确认保存表单 / 执行操作</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘</span>
                  <span class="m26-kbd-badge">↩</span>
                </div>
              </div>
              <div class="m26-shortcut-row">
                <span class="m26-shortcut-desc">全选 / 复制 / 粘贴 / 撤销</span>
                <div class="m26-kbd-keys">
                  <span class="m26-kbd-badge">⌘A</span>
                  <span class="m26-kbd-badge">⌘C</span>
                  <span class="m26-kbd-badge">⌘V</span>
                  <span class="m26-kbd-badge">⌘Z</span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-action-btn primary" @click="showShortcutsModal = false">知道了</button>
        </div>
      </div>
    </div>

    <!-- MODAL: CLAUDE CONFIG -->
    <div v-if="showClaudeConfigModal" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showClaudeConfigModal = false)">
      <div class="m26-modal-sheet m26-client-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <span class="m26-client-modal-icon claude">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 2v20M2 12h20M4.93 4.93l14.14 14.14M4.93 19.07l14.14-14.14"/>
              </svg>
            </span>
            <div>
              <h3 class="m26-modal-title">配置 Claude 桌面端</h3>
              <p class="m26-modal-subtitle">挑选可用模型并映射到 Claude 桌面端槽位，支持 1M 超长上下文与流式自动转译</p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="showClaudeConfigModal = false">✕</button>
        </div>

        <div class="m26-modal-body">
          <!-- Step 1: Model Selection -->
          <div class="m26-client-cfg-section">
            <div class="flex items-center justify-between mb-2">
              <span class="m26-label font-semibold">1. 勾选要同步的模型 ({{ claudeSelectedModels.length }}/{{ allAvailableModels.length }})</span>
              <div class="flex items-center gap-2">
                <button class="m26-text-btn" @click="selectAllClaudeModels">全选</button>
                <span class="text-slate-300">·</span>
                <button class="m26-text-btn" @click="clearClaudeModels">清空</button>
              </div>
            </div>
            <div class="m26-client-model-picker">
              <div
                v-for="model in allAvailableModels"
                :key="model"
                class="m26-model-pick-item"
                :class="{ selected: claudeSelectedModels.includes(model) }"
                @click="toggleClaudeModel(model)"
              >
                <span class="m26-checkbox-custom" :class="{ checked: claudeSelectedModels.includes(model) }"></span>
                <span class="font-mono text-xs text-main truncate flex-1" :title="model">{{ model }}</span>
                <span class="m26-model-chan-badge" :class="getModelHealth(model).statusClass" :title="getModelHealth(model).tooltip">
                  <span class="m26-status-dot-sm" :class="getModelHealth(model).statusClass"></span>
                  <span class="m26-chan-name">{{ getModelHealth(model).channelName }}</span>
                  <span v-if="getModelHealth(model).latencyText" class="m26-chan-latency">{{ getModelHealth(model).latencyText }}</span>
                </span>
              </div>
              <div v-if="allAvailableModels.length === 0" class="text-xs text-slate-400 py-3 text-center">
                尚未在“供应商”中配置任何可用模型，请先添加渠道模型。
              </div>
            </div>
          </div>

          <!-- Step 2: Slot Mapping -->
          <div class="m26-client-cfg-section mt-4">
            <div class="flex items-center justify-between mb-2">
              <span class="m26-label font-semibold">2. 槽位与映射关系 (映射至 Claude 客户端下拉列表)</span>
              <button class="m26-text-btn" @click="autoAssignClaudeSlots">自动按健康模型优先分配</button>
            </div>
            <div class="m26-slot-list">
              <div v-for="item in claudeMappings" :key="item.slot" class="m26-slot-row">
                <div class="m26-slot-info">
                  <span class="m26-slot-tag">{{ item.slot }}</span>
                  <span class="m26-slot-label">{{ item.slotLabel }}</span>
                </div>
                <div class="flex items-center gap-2 flex-1 justify-end">
                  <select v-model="item.upstreamModel" class="m26-select text-xs flex-1 max-w-[240px]">
                    <option value="">-- 未指定 (自动回退) --</option>
                    <option v-for="m in (claudeSelectedModels.length > 0 ? claudeSelectedModels : allAvailableModels)" :key="m" :value="m">
                      {{ m }} ({{ getModelHealth(m).channelName }} · {{ getModelHealth(m).status === 'healthy' ? '正常' : (getModelHealth(m).status === 'failed' ? '异常' : '就绪') }})
                    </option>
                  </select>
                  <label class="flex items-center gap-1 text-xs text-slate-500 cursor-pointer" title="声明支持 1M Context Window">
                    <input type="checkbox" v-model="item.supports1m" class="rounded" />
                    <span>1M</span>
                  </label>
                </div>
              </div>
            </div>
          </div>

          <!-- Notice -->
          <div class="m26-client-notice-box mt-3">
            <div class="flex items-start gap-2">
              <span class="text-indigo-500 mt-0.5">ℹ️</span>
              <span class="text-xs text-slate-500 leading-relaxed">
                保存后将写入 <code>~/Library/Application Support/Claude-3p/configLibrary</code> 并配置为 3P 扩展模式。
                HeiGate 网关会自动将 Anthropic 请求智能转译给上游对应模型（即便上游是 OpenAI 协议），并还原实时流式打字效果。
              </span>
            </div>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-action-btn secondary" @click="showClaudeConfigModal = false">取消</button>
          <button class="m26-action-btn" :disabled="isSavingClaudeConfig" @click="handleSaveClaudeConfig(false)">
            {{ isSavingClaudeConfig ? '保存中...' : '写入配置' }}
          </button>
          <button class="m26-action-btn primary" :disabled="isSavingClaudeConfig || isLaunchingClaude" @click="handleSaveClaudeConfig(true)">
            {{ isSavingClaudeConfig ? '正在保存...' : '写入并立即启动' }}
          </button>
        </div>
      </div>
    </div>

    <!-- MODAL: CODEX CONFIG -->
    <div v-if="showCodexConfigModal" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showCodexConfigModal = false)">
      <div class="m26-modal-sheet m26-client-sheet">
        <div class="m26-modal-header" @mousedown="onToolbarMouseDown">
          <div class="flex items-center gap-2">
            <span class="m26-client-modal-icon codex">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="16 18 22 12 16 6"/>
                <polyline points="8 6 2 12 8 18"/>
              </svg>
            </span>
            <div>
              <h3 class="m26-modal-title">配置 Codex 桌面端 (ChatGPT / Codex CLI)</h3>
              <p class="m26-modal-subtitle">写入 ~/.codex/config.toml 并生成专属 heigate-model-catalog.json</p>
            </div>
          </div>
          <button class="m26-close-btn" data-no-drag @click="showCodexConfigModal = false">✕</button>
        </div>

        <div class="m26-modal-body">
          <!-- Step 1: Model Selection -->
          <div class="m26-client-cfg-section">
            <div class="flex items-center justify-between mb-2">
              <span class="m26-label font-semibold">1. 勾选要同步至 Codex 模型的列表 ({{ codexSelectedModels.length }}/{{ allAvailableModels.length }})</span>
              <div class="flex items-center gap-2">
                <button class="m26-text-btn" @click="selectAllCodexModels">全选</button>
                <span class="text-slate-300">·</span>
                <button class="m26-text-btn" @click="clearCodexModels">清空</button>
              </div>
            </div>
            <div class="m26-client-model-picker">
              <div
                v-for="model in allAvailableModels"
                :key="model"
                class="m26-model-pick-item"
                :class="{ selected: codexSelectedModels.includes(model) }"
                @click="toggleCodexModel(model)"
              >
                <span class="m26-checkbox-custom" :class="{ checked: codexSelectedModels.includes(model) }"></span>
                <span class="font-mono text-xs text-main truncate flex-1" :title="model">{{ model }}</span>
                <span class="m26-model-chan-badge" :class="getModelHealth(model).statusClass" :title="getModelHealth(model).tooltip">
                  <span class="m26-status-dot-sm" :class="getModelHealth(model).statusClass"></span>
                  <span class="m26-chan-name">{{ getModelHealth(model).channelName }}</span>
                  <span v-if="getModelHealth(model).latencyText" class="m26-chan-latency">{{ getModelHealth(model).latencyText }}</span>
                </span>
              </div>
              <div v-if="allAvailableModels.length === 0" class="text-xs text-slate-400 py-3 text-center">
                尚未在“供应商”中配置任何可用模型，请先添加渠道模型。
              </div>
            </div>
          </div>

          <!-- Step 2: Default Model -->
          <div class="m26-client-cfg-section mt-4">
            <span class="m26-label font-semibold mb-2 block">2. 设定默认主启动模型 (Default Model)</span>
            <select v-model="codexDefaultModel" class="m26-select w-full">
              <option v-for="m in (codexSelectedModels.length > 0 ? codexSelectedModels : allAvailableModels)" :key="m" :value="m">
                {{ m }} ({{ getModelHealth(m).channelName }} · {{ getModelHealth(m).status === 'healthy' ? '正常' : (getModelHealth(m).status === 'failed' ? '异常' : '就绪') }})
              </option>
            </select>
          </div>

          <!-- Notice -->
          <div class="m26-client-notice-box mt-3">
            <div class="flex items-start gap-2">
              <span class="text-emerald-500 mt-0.5">ℹ️</span>
              <span class="text-xs text-slate-500 leading-relaxed">
                配置将安全增量写入 <code>~/.codex/config.toml</code>（原配置自动备份为 <code>.bak.heigate</code>），
                并写入 <code>~/.codex/auth.json</code> 与模型字典。ChatGPT 桌面端或 <code>codex</code> 命令行将直接经由本地网关极速转发与分流。
              </span>
            </div>
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-action-btn secondary" @click="showCodexConfigModal = false">取消</button>
          <button class="m26-action-btn" :disabled="isSavingCodexConfig" @click="handleSaveCodexConfig(false)">
            {{ isSavingCodexConfig ? '保存中...' : '写入配置' }}
          </button>
          <button class="m26-action-btn primary" :disabled="isSavingCodexConfig || isLaunchingCodex" @click="handleSaveCodexConfig(true)">
            {{ isSavingCodexConfig ? '正在保存...' : '写入并立即启动' }}
          </button>
        </div>
      </div>
    </div>

    <!-- Floating Global Toast -->
    <Transition name="m26-toast">
      <div v-if="toastMessage" class="m26-toast-pill">
        <span class="m26-toast-beacon"></span>
        <span class="m26-toast-text">{{ toastMessage }}</span>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* ==========================================================================
   macOS 26 Pure Light / White Theme System
   ========================================================================== */
.m26-window {
  --m26-bg: #f8fafc;
  --m26-sidebar-bg: rgba(248, 250, 252, 0.82);
  --m26-header-bg: rgba(255, 255, 255, 0.82);
  --m26-card-bg: rgba(255, 255, 255, 0.9);
  --m26-card-hover: #ffffff;
  --m26-border: rgba(0, 0, 0, 0.07);
  --m26-border-bright: rgba(0, 0, 0, 0.12);
  --m26-text: #0f172a;
  --m26-text-secondary: #475569;
  --m26-text-muted: #94a3b8;
  --m26-accent: #4f46e5;
  --m26-accent-glow: rgba(79, 70, 229, 0.15);
  --m26-emerald: #059669;
  --m26-amber: #d97706;
  --m26-rose: #e11d48;
  --m26-cyan: #0891b2;

  position: relative;
  display: flex;
  flex-direction: column;
  width: 100vw;
  height: 100vh;
  min-width: 1000px;
  min-height: 680px;
  overflow: hidden;
  background-color: var(--m26-bg);
  color: var(--m26-text);
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Display", "SF Pro Text", "Helvetica Neue", "PingFang SC", sans-serif;
  user-select: text;
  -webkit-user-select: text;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

/* Native macOS accent-tinted text selection */
::selection {
  background: rgba(79, 70, 229, 0.22);
  color: inherit;
}
::-moz-selection {
  background: rgba(79, 70, 229, 0.22);
  color: inherit;
}

/* Keep interactive UI chrome non-selectable */
.m26-app-titlebar,
.m26-titlebar-drag-area,
.m26-titlebar-nav,
.m26-nav-tab,
.m26-seg-btn,
.m26-card-switch,
.m26-card-btn,
.m26-action-btn,
.m26-glass-btn,
.m26-close-btn,
.m26-traffic-lights-spacer,
.m26-modal-header {
  user-select: none;
  -webkit-user-select: none;
}

/* Ambient dynamic background in delicate light pastels */
.m26-ambient-layer {
  position: absolute;
  inset: 0;
  pointer-events: none;
  overflow: hidden;
  z-index: 0;
}
.m26-glow-orb {
  position: absolute;
  border-radius: 50%;
  filter: blur(90px);
  opacity: 0.6;
}
.orb-1 {
  width: 550px;
  height: 550px;
  background: radial-gradient(circle, rgba(99, 102, 241, 0.08) 0%, transparent 70%);
  top: -150px;
  left: 80px;
}
.orb-2 {
  width: 500px;
  height: 500px;
  background: radial-gradient(circle, rgba(16, 185, 129, 0.06) 0%, transparent 70%);
  bottom: -100px;
  right: 120px;
}
.orb-3 {
  width: 400px;
  height: 400px;
  background: radial-gradient(circle, rgba(6, 182, 212, 0.05) 0%, transparent 70%);
  top: 35%;
  left: 40%;
}

/* ==========================================================================
   macOS 26 Top Navigation Bar (Floating Pill Capsule Bar)
   ========================================================================== */
.m26-top-navbar {
  position: relative;
  z-index: 50;
  width: 100%;
  height: 96px;
  min-height: 96px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 30px 24px 10px 24px;
  background: transparent;
  backdrop-filter: none;
  -webkit-backdrop-filter: none;
  border-bottom: none;
  box-shadow: none;
}

.m26-nav-left {
  flex: 0 0 100px;
  min-width: 80px;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  z-index: 20;
}

.m26-traffic-lights {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 14px;
  padding-left: 2px;
}
.m26-traffic-lights .light {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: 0.5px solid rgba(0, 0, 0, 0.12);
  box-shadow: inset 0 1px 1px rgba(255, 255, 255, 0.5);
}
.m26-traffic-lights .light.red { background: #ff5f57; }
.m26-traffic-lights .light.yellow { background: #febc2e; }
.m26-traffic-lights .light.green { background: #28c840; }

.m26-native-titlebar-spacer {
  width: 76px;
  height: 100%;
  flex-shrink: 0;
}

/* --------------------------------------------------------------------------
   Center: Floating Pill Segmented Capsule Navigation
   -------------------------------------------------------------------------- */
.m26-nav-center {
  flex: 1;
  display: flex;
  justify-content: center;
  align-items: center;
  min-width: 0;
  padding: 0 12px;
  container-type: inline-size;
}

.m26-floating-nav {
  position: relative;
  display: inline-flex;
  align-items: center;
  padding: 5px 6px;
  background: #ffffff;
  border-radius: 9999px;
  border: 1px solid rgba(0, 0, 0, 0.08);
  box-shadow: 0 4px 18px -2px rgba(0, 0, 0, 0.06), 0 1px 4px rgba(0, 0, 0, 0.03);
  gap: 4px;
  white-space: nowrap;
  flex-shrink: 0;
  z-index: 10;
}

.m26-pill-item {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  height: 38px;
  padding: 0 16px;
  border-radius: 9999px;
  font-size: 13.5px;
  font-weight: 500;
  color: var(--m26-text-secondary);
  cursor: pointer;
  transition: all 0.16s cubic-bezier(0.16, 1, 0.3, 1);
  border: none;
  background: transparent;
  user-select: none;
  white-space: nowrap;
  flex-shrink: 0;
}

.m26-pill-text {
  display: inline-block;
  white-space: nowrap;
  line-height: 1;
}

.m26-pill-icon {
  width: 17px;
  height: 17px;
  stroke-width: 1.9;
  color: currentColor;
  transition: transform 0.16s ease;
  flex-shrink: 0;
}

.m26-pill-item:hover {
  background: rgba(0, 0, 0, 0.04);
  color: var(--m26-text);
}

.m26-pill-item.active {
  background: #eef2ff;
  color: #4338ca;
  font-weight: 600;
  box-shadow: inset 0 0 0 1px rgba(79, 70, 229, 0.16);
}

.m26-pill-item.active .m26-pill-icon {
  color: #4f46e5;
}

.m26-pill-badge {
  font-size: 11px;
  font-weight: 600;
  padding: 0 6px;
  height: 18px;
  line-height: 18px;
  border-radius: 9px;
  background: rgba(0, 0, 0, 0.06);
  color: var(--m26-text-muted);
  flex-shrink: 0;
}

.m26-pill-item.active .m26-pill-badge {
  background: rgba(79, 70, 229, 0.15);
  color: #4338ca;
}

/* 响应式收缩：只有在极端狭窄时（< 750px）才隐藏文字留图标 */
.m26-floating-nav.compact-tabs .m26-pill-text {
  display: none !important;
}
.m26-floating-nav.compact-tabs .m26-pill-item {
  padding: 0 11px;
  gap: 4px;
  height: 38px;
}
.m26-floating-nav.compact-tabs .m26-pill-icon {
  width: 17px;
  height: 17px;
}
.m26-floating-nav.compact-tabs .m26-pill-badge {
  margin-left: 2px;
  font-size: 10px;
  padding: 0 5px;
  height: 16px;
  line-height: 16px;
}

@container (max-width: 480px) {
  .m26-floating-nav .m26-pill-text {
    display: none !important;
  }
  .m26-floating-nav .m26-pill-item {
    padding: 0 11px;
    gap: 4px;
    height: 38px;
  }
  .m26-floating-nav .m26-pill-icon {
    width: 17px;
    height: 17px;
  }
  .m26-floating-nav .m26-pill-badge {
    margin-left: 2px;
    font-size: 10px;
    padding: 0 5px;
    height: 16px;
    line-height: 16px;
  }
}

@media (max-width: 750px) {
  .m26-floating-nav .m26-pill-text {
    display: none !important;
  }
  .m26-floating-nav .m26-pill-item {
    padding: 0 11px;
    gap: 4px;
    height: 38px;
  }
}

/* --------------------------------------------------------------------------
   Right: Quick Tools & Actions
   -------------------------------------------------------------------------- */
.m26-nav-right {
  flex: 0 0 100px;
  min-width: 80px;
  display: flex;
  align-items: center;
  gap: 8px;
  justify-content: flex-end;
  z-index: 20;
}

.m26-gateway-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  background: rgba(5, 150, 105, 0.08);
  border: 1px solid rgba(5, 150, 105, 0.2);
  border-radius: 20px;
  font-size: 11.5px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  color: #047857;
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-gateway-chip:hover {
  background: rgba(5, 150, 105, 0.15);
  border-color: rgba(5, 150, 105, 0.35);
}

.m26-gateway-chip.offline {
  background: rgba(225, 29, 72, 0.08);
  border-color: rgba(225, 29, 72, 0.2);
  color: var(--m26-rose);
}

.m26-chip-pulse {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--m26-emerald);
  box-shadow: 0 0 6px rgba(5, 150, 105, 0.6);
}

.m26-gateway-chip.offline .m26-chip-pulse {
  background: var(--m26-rose);
  box-shadow: 0 0 6px rgba(225, 29, 72, 0.6);
}

/* --------------------------------------------------------------------------
   Toolbar Search Component (in Section Toolbars)
   -------------------------------------------------------------------------- */
.m26-toolbar-search {
  position: relative;
  display: flex;
  align-items: center;
}

.m26-toolbar-search .m26-search-icon {
  position: absolute;
  left: 10px;
  color: var(--m26-text-muted);
  pointer-events: none;
}

.m26-toolbar-search-input {
  width: 175px;
  height: 32px;
  padding: 0 28px 0 30px;
  font-size: 12px;
  border-radius: 8px;
  border: 1px solid var(--m26-border);
  background: #ffffff;
  color: var(--m26-text);
  outline: none;
  transition: all 0.18s ease;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
}

.m26-toolbar-search-input:focus {
  width: 210px;
  border-color: var(--m26-accent);
  box-shadow: 0 0 0 3px var(--m26-accent-glow), 0 1px 2px rgba(0, 0, 0, 0.05);
}

.m26-toolbar-search .m26-search-clear {
  position: absolute;
  right: 8px;
  background: transparent;
  border: none;
  color: var(--m26-text-muted);
  font-size: 11px;
  cursor: pointer;
  padding: 2px;
  line-height: 1;
}

.m26-toolbar-search .m26-search-clear:hover {
  color: var(--m26-text);
}

.m26-toolbar-search .m26-kbd {
  position: absolute;
  right: 7px;
  pointer-events: none;
}

.m26-header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.m26-live-indicator {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--m26-text-muted);
  transition: all 0.2s;
}

.m26-live-indicator.live {
  background: var(--m26-emerald);
  box-shadow: 0 0 6px rgba(5, 150, 105, 0.7);
}

/* --------------------------------------------------------------------------
   Workspace & Viewport Full Width
   -------------------------------------------------------------------------- */
.m26-workspace {
  position: relative;
  z-index: 1;
  flex: 1;
  width: 100%;
  height: calc(100vh - 96px);
  min-height: 0;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.m26-viewport {
  flex: 1;
  width: 100%;
  height: 100%;
  overflow-y: auto;
  padding: 24px 32px 60px;
  max-width: 1440px;
  margin: 0 auto;
}

.m26-kbd {
  font-size: 10px;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.06);
  border-radius: 4px;
  padding: 2px 4px;
  font-family: ui-monospace, SFMono-Regular, monospace;
}

/* Buttons */
.m26-action-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 14px;
  border-radius: 9px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.16s ease;
}
.m26-action-btn.primary {
  background: linear-gradient(135deg, #4f46e5, #4338ca);
  border: 1px solid rgba(255, 255, 255, 0.3);
  color: #fff;
  box-shadow: 0 2px 8px rgba(79, 70, 229, 0.28);
}
.m26-action-btn.primary:hover {
  filter: brightness(1.08);
  box-shadow: 0 4px 14px rgba(79, 70, 229, 0.4);
  transform: translateY(-1px);
}
.m26-action-btn.primary:active {
  transform: translateY(0);
}

.m26-glass-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 12px;
  border-radius: 8px;
  background: #ffffff;
  border: 1px solid var(--m26-border-bright);
  color: var(--m26-text-secondary);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: all 0.15s ease;
}
.m26-glass-btn:hover {
  background: #f8fafc;
  color: var(--m26-text);
  border-color: rgba(0, 0, 0, 0.18);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.05);
}
.m26-glass-btn.danger {
  color: var(--m26-rose);
  border-color: rgba(225, 29, 72, 0.25);
  background: #fff;
}
.m26-glass-btn.danger:hover {
  background: rgba(225, 29, 72, 0.08);
  border-color: var(--m26-rose);
}

.m26-link-btn {
  background: transparent;
  border: none;
  color: var(--m26-accent);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: opacity 0.15s ease;
}
.m26-link-btn:hover {
  opacity: 0.8;
}

/* ==========================================================================
   Viewport Sections
   ========================================================================== */
.m26-viewport {
  flex: 1;
  overflow-y: auto;
  padding: 24px 28px 48px;
}
.m26-section-view {
  display: flex;
  flex-direction: column;
  gap: 24px;
  max-width: 1400px;
  margin: 0 auto;
}

/* Bento Grid */
.m26-bento-grid {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 16px;
}
.m26-bento-card {
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  border: 1px solid var(--m26-border);
  border-radius: 16px;
  padding: 18px 20px;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.02), inset 0 1px 0 0 rgba(255, 255, 255, 0.9);
  transition: all 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}
.m26-bento-card:hover {
  background: var(--m26-card-hover);
  border-color: var(--m26-border-bright);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.05), inset 0 1px 0 0 rgba(255, 255, 255, 1);
  transform: translateY(-2px);
}
.m26-bento-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.m26-bento-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--m26-text-secondary);
}
.m26-pill-icon {
  width: 26px;
  height: 26px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.m26-pill-icon.emerald { background: rgba(5, 150, 105, 0.12); color: var(--m26-emerald); }
.m26-pill-icon.indigo { background: rgba(79, 70, 229, 0.12); color: var(--m26-accent); }
.m26-pill-icon.amber { background: rgba(217, 119, 6, 0.12); color: var(--m26-amber); }
.m26-pill-icon.cyan { background: rgba(8, 145, 178, 0.12); color: var(--m26-cyan); }

.m26-bento-stat {
  margin: 14px 0 10px;
  display: flex;
  align-items: baseline;
  gap: 4px;
}
.m26-bento-num {
  font-size: 32px;
  font-weight: 800;
  letter-spacing: -0.03em;
  color: var(--m26-text);
  font-variant-numeric: tabular-nums;
}
.m26-bento-sub {
  font-size: 14px;
  color: var(--m26-text-muted);
}
.m26-bento-unit {
  font-size: 16px;
  color: var(--m26-text-secondary);
  font-weight: 600;
}
.m26-bento-text {
  font-size: 20px;
  font-weight: 700;
  color: var(--m26-text);
}
.m26-bento-foot {
  display: flex;
  align-items: center;
}
.m26-tag-subtle {
  font-size: 11px;
  color: var(--m26-text-muted);
}

/* Quick Bar */
.m26-quick-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: rgba(255, 255, 255, 0.7);
  border: 1px solid var(--m26-border);
  border-radius: 12px;
  padding: 10px 14px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
}
.m26-bar-left {
  display: flex;
  gap: 10px;
}

/* Mini Grid in Overview */
.m26-card-section {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.m26-section-title {
  font-size: 13px;
  font-weight: 700;
  color: var(--m26-text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.m26-mini-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 12px;
}
.m26-mini-card {
  background: var(--m26-card-bg);
  border: 1px solid var(--m26-border);
  border-radius: 12px;
  padding: 12px 14px;
  cursor: pointer;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.02);
  transition: all 0.16s ease;
}
.m26-mini-card:hover {
  background: var(--m26-card-hover);
  border-color: var(--m26-border-bright);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.05);
  transform: translateY(-1px);
}
.m26-mini-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.m26-mini-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--m26-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.m26-mini-meta {
  display: flex;
  justify-content: space-between;
  font-size: 11px;
  color: var(--m26-text-muted);
}
.m26-mini-lat {
  font-family: ui-monospace, SFMono-Regular, monospace;
}

/* ==========================================================================
   Section 2: Channels Management
   ========================================================================== */
.m26-control-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}
.m26-segmented {
  display: inline-flex;
  background: rgba(0, 0, 0, 0.05);
  border: 1px solid var(--m26-border);
  border-radius: 9px;
  padding: 3px;
  gap: 2px;
}
.m26-seg-btn {
  background: transparent;
  border: none;
  border-radius: 7px;
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 500;
  color: var(--m26-text-secondary);
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-seg-btn:hover {
  color: var(--m26-text);
}
.m26-seg-btn.active {
  background: #ffffff;
  color: var(--m26-text);
  font-weight: 600;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.08);
}
.m26-seg-btn.icon-only {
  padding: 6px 9px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.m26-tools-group,
.m26-tools-left {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.m26-tools-right {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

/* Channel Card Grid */
.m26-channels-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
  gap: 18px;
  align-items: stretch;
}

/* Modern macOS 26 White Channel Card */
.m26-hg-card {
  background: #ffffff;
  border-radius: 18px;
  border: 1px solid rgba(0, 0, 0, 0.08);
  box-shadow: 0 4px 18px -2px rgba(0, 0, 0, 0.04), 0 1px 3px rgba(0, 0, 0, 0.02);
  padding: 16px 18px 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  transition: all 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  position: relative;
  overflow: hidden;
  height: 100%;
}
.m26-hg-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 10px 28px -4px rgba(0, 0, 0, 0.07), 0 2px 8px rgba(0, 0, 0, 0.03);
  border-color: rgba(79, 70, 229, 0.25);
}
.m26-hg-card.disabled {
  opacity: 0.65;
  filter: grayscale(0.2);
}

/* Card Header */
.m26-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}
.m26-card-title-group {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.m26-priority-badge {
  font-size: 11px;
  font-weight: 700;
  padding: 2px 7px;
  border-radius: 6px;
  background: rgba(79, 70, 229, 0.08);
  color: #4f46e5;
  font-family: ui-monospace, SFMono-Regular, monospace;
  flex-shrink: 0;
}
.m26-card-name {
  font-size: 15px;
  font-weight: 700;
  color: #0f172a;
  letter-spacing: -0.01em;
  margin: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.m26-card-badges-group {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}
.m26-card-status-pill {
  font-size: 11px;
  font-weight: 600;
  padding: 2px 8px;
  border-radius: 6px;
  line-height: 1.4;
}
.m26-card-status-pill.healthy {
  background: #10b981;
  color: #ffffff;
}
.m26-card-status-pill.degraded {
  background: #f59e0b;
  color: #ffffff;
}
.m26-card-status-pill.offline {
  background: #f43f5e;
  color: #ffffff;
}
.m26-card-proto-pill {
  font-size: 11px;
  font-weight: 600;
  padding: 2px 8px;
  border-radius: 6px;
  line-height: 1.4;
  background: #e0f2fe;
  color: #0284c7;
}
.m26-card-proto-pill.anthropic {
  background: #fef3c7;
  color: #b45309;
}
.m26-card-proto-pill.openai-responses {
  background: #e0e7ff;
  color: #4338ca;
}

/* Card Switch */
.m26-card-switch {
  position: relative;
  display: inline-block;
  width: 32px;
  height: 18px;
  cursor: pointer;
  margin-left: 2px;
}
.m26-card-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}
.m26-card-switch .slider {
  position: absolute;
  cursor: pointer;
  inset: 0;
  background-color: rgba(0, 0, 0, 0.12);
  transition: 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  border-radius: 20px;
}
.m26-card-switch .slider:before {
  position: absolute;
  content: "";
  height: 14px;
  width: 14px;
  left: 2px;
  bottom: 2px;
  background-color: white;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  transition: 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  border-radius: 50%;
}
.m26-card-switch input:checked + .slider {
  background-color: #10b981;
}
.m26-card-switch input:checked + .slider:before {
  transform: translateX(14px);
}

/* Service / Channel Tag Row */
.m26-card-tag-row {
  display: flex;
  align-items: center;
}
.m26-service-tag {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  border-radius: 6px;
  background: #eef2ff;
  color: #4338ca;
  font-size: 11px;
  font-weight: 600;
  width: fit-content;
}
.m26-service-tag.warning {
  background: #fff1f2;
  color: #e11d48;
}

/* Card Models List - 1 row per model, left name, right latency / error */
.m26-card-models-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
  background: #f8fafc;
  border-radius: 12px;
  border: 1px solid rgba(0, 0, 0, 0.05);
  flex: 1;
}
.m26-models-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.m26-models-title {
  font-size: 11px;
  font-weight: 600;
  color: #64748b;
  letter-spacing: 0.02em;
}
.m26-model-rows {
  display: flex;
  flex-direction: column;
  gap: 5px;
  flex: 1;
}
.m26-model-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 5px 8px;
  border-radius: 7px;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.05);
  min-height: 28px;
  transition: all 0.15s ease;
}
.m26-model-row:hover {
  border-color: rgba(79, 70, 229, 0.25);
  background: #fafafa;
}
.m26-model-row.has-error {
  border-color: rgba(239, 68, 68, 0.25);
  background: rgba(254, 242, 242, 0.5);
}
.m26-model-row-name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex: 1;
}
.m26-model-bullet {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #10b981;
  flex-shrink: 0;
}
.m26-model-bullet.error {
  background: #ef4444;
}
.m26-model-name-text {
  font-size: 12px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-weight: 600;
  color: #1e293b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.m26-model-row-latency {
  display: flex;
  align-items: center;
  flex-shrink: 0;
}

/* Latency Badge */
.m26-model-lat-tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  font-weight: 600;
  padding: 2px 7px;
  border-radius: 6px;
  cursor: pointer;
  background: #f1f5f9;
  color: #475569;
  border: 1px solid rgba(0, 0, 0, 0.04);
  transition: all 0.15s ease;
}
.m26-model-lat-tag:hover {
  opacity: 0.85;
}
.m26-model-lat-tag.fast {
  background: #ecfdf5;
  color: #059669;
  border-color: rgba(16, 185, 129, 0.2);
}
.m26-model-lat-tag.normal {
  background: #eff6ff;
  color: #2563eb;
  border-color: rgba(37, 99, 235, 0.2);
}
.m26-model-lat-tag.slow {
  background: #fffbeb;
  color: #d97706;
  border-color: rgba(217, 119, 6, 0.2);
}
.m26-model-lat-tag.probing {
  background: #f5f3ff;
  color: #7c3aed;
  border-color: rgba(124, 58, 237, 0.2);
}
.m26-model-lat-tag.timeout,
.m26-model-lat-tag.offline {
  background: #fef2f2;
  color: #dc2626;
  border-color: rgba(239, 68, 68, 0.2);
}

/* Error Badge */
.m26-model-err-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 6px;
  font-size: 11px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  font-weight: 700;
  background: #fee2e2;
  color: #dc2626;
  border: 1px solid rgba(220, 38, 38, 0.3);
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-model-err-badge:hover {
  background: #fecaca;
  transform: scale(1.02);
}
.m26-err-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #dc2626;
}
.m26-err-arrow {
  font-size: 9px;
  opacity: 0.8;
  margin-left: 2px;
}

/* More / Expand Button */
.m26-model-more-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  margin-top: auto;
  border-radius: 8px;
  font-size: 11px;
  font-weight: 600;
  color: #4f46e5;
  background: rgba(79, 70, 229, 0.06);
  border: 1px dashed rgba(79, 70, 229, 0.25);
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-model-more-btn:hover {
  background: rgba(79, 70, 229, 0.12);
  border-color: rgba(79, 70, 229, 0.45);
  color: #4338ca;
}

/* Card Action Buttons (探活, 编辑, 复制, 删除) - pinned to bottom */
.m26-card-actions {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 6px;
  padding-top: 10px;
  border-top: 1px solid rgba(0, 0, 0, 0.06);
  margin-top: auto;
}
.m26-card-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 30px;
  padding: 0 4px;
  border-radius: 7px;
  font-size: 11.5px;
  font-weight: 500;
  color: #475569;
  background: #f8fafc;
  border: 1px solid rgba(0, 0, 0, 0.08);
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}
.m26-card-btn:hover {
  color: #0f172a;
  background: #f1f5f9;
  border-color: rgba(0, 0, 0, 0.15);
}
.m26-card-btn.danger {
  color: #64748b;
}
.m26-card-btn.danger:hover {
  color: #e11d48;
  background: #fff1f2;
  border-color: rgba(225, 29, 72, 0.3);
}
.m26-card-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.m26-card-btn svg {
  flex-shrink: 0;
}
.m26-ch-tag-group {
  display: flex;
  align-items: center;
  gap: 6px;
}
.m26-priority-pill {
  font-size: 10px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 5px;
  background: rgba(0, 0, 0, 0.05);
  color: var(--m26-text-secondary);
}
.m26-proto-pill {
  font-size: 10px;
  font-weight: 600;
  padding: 2px 7px;
  border-radius: 5px;
  text-transform: uppercase;
}
.m26-proto-pill.anthropic {
  background: rgba(217, 119, 6, 0.12);
  color: var(--m26-amber);
}
.m26-proto-pill.openai-compatible {
  background: rgba(5, 150, 105, 0.12);
  color: var(--m26-emerald);
}
.m26-proto-pill.openai-responses {
  background: rgba(37, 99, 235, 0.12);
  color: #2563eb;
}
.m26-interval-pill {
  font-size: 10px;
  font-weight: 500;
  padding: 2px 6px;
  border-radius: 5px;
  background: rgba(0, 0, 0, 0.04);
  color: var(--m26-text-muted);
}
.m26-circuit-pill {
  font-size: 10px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 5px;
}
.m26-circuit-pill.open {
  background: rgba(225, 29, 72, 0.12);
  color: var(--m26-rose);
}
.m26-circuit-pill.half_open {
  background: rgba(217, 119, 6, 0.12);
  color: var(--m26-amber);
}

.m26-ch-name {
  font-size: 15px;
  font-weight: 700;
  color: var(--m26-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.m26-ch-url {
  font-size: 11px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  color: var(--m26-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  margin-top: 2px;
}

.m26-ch-telemetry {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  background: #f8fafc;
  border: 1px solid rgba(0, 0, 0, 0.04);
  border-radius: 8px;
}
.m26-ch-status-box {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 500;
  cursor: help;
  white-space: nowrap;
}
.m26-status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--m26-rose);
}
.m26-status-dot.healthy { background: var(--m26-emerald); box-shadow: 0 0 6px rgba(5, 150, 105, 0.4); }
.m26-status-dot.degraded { background: var(--m26-amber); box-shadow: 0 0 6px rgba(217, 119, 6, 0.4); }
.m26-status-dot.offline { background: var(--m26-rose); }
.m26-status-dot.unknown { background: #94a3b8; }

.m26-ch-latency-box {
  display: flex;
  align-items: center;
  gap: 6px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  font-size: 12px;
  color: var(--m26-text-secondary);
}
.m26-signal-bars {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 10px;
}
.m26-signal-bars span {
  width: 2.5px;
  background: rgba(0, 0, 0, 0.12);
  border-radius: 1px;
}
.m26-signal-bars span:nth-child(1) { height: 4px; }
.m26-signal-bars span:nth-child(2) { height: 7px; }
.m26-signal-bars span:nth-child(3) { height: 10px; }
.m26-signal-bars.sig-3 span { background: var(--m26-emerald); }
.m26-signal-bars.sig-2 span:nth-child(1), .m26-signal-bars.sig-2 span:nth-child(2) { background: var(--m26-amber); }
.m26-signal-bars.sig-1 span:nth-child(1) { background: var(--m26-rose); }

.m26-ch-models-strip {
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
}
.m26-model-badge {
  font-size: 11px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  padding: 2px 7px;
  border-radius: 6px;
  white-space: nowrap;
}
.m26-model-badge.primary {
  background: rgba(0, 0, 0, 0.05);
  color: var(--m26-text-secondary);
}
.m26-model-badge.count {
  background: rgba(79, 70, 229, 0.1);
  color: var(--m26-accent);
  font-weight: 600;
}

.m26-ch-card-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-top: 8px;
  border-top: 1px solid var(--m26-border);
}
.m26-btn-micro {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  height: 26px;
  padding: 0 10px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
  background: #ffffff;
  border: 1px solid var(--m26-border-bright);
  color: var(--m26-text);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: all 0.15s ease;
  white-space: nowrap;
  flex-shrink: 0;
}
.m26-btn-micro:hover {
  background: #f8fafc;
  border-color: rgba(0, 0, 0, 0.18);
  box-shadow: 0 2px 5px rgba(0, 0, 0, 0.05);
}
.m26-btn-group {
  display: flex;
  align-items: center;
  gap: 3px;
}
.m26-btn-icon {
  width: 26px;
  height: 26px;
  border-radius: 6px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 1px solid transparent;
  color: var(--m26-text-muted);
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;
}
.m26-btn-icon:hover {
  background: rgba(0, 0, 0, 0.05);
  color: var(--m26-text);
  border-color: var(--m26-border);
}
.m26-btn-icon.danger:hover {
  background: rgba(225, 29, 72, 0.1);
  color: var(--m26-rose);
  border-color: rgba(225, 29, 72, 0.2);
}

/* Switch */
.m26-switch {
  position: relative;
  display: inline-block;
  width: 34px;
  height: 20px;
}
.m26-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}
.m26-slider {
  position: absolute;
  cursor: pointer;
  inset: 0;
  background-color: rgba(0, 0, 0, 0.12);
  transition: 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  border-radius: 20px;
}
.m26-slider:before {
  position: absolute;
  content: "";
  height: 16px;
  width: 16px;
  left: 2px;
  bottom: 2px;
  background-color: white;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  transition: 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  border-radius: 50%;
}
input:checked + .m26-slider {
  background-color: var(--m26-emerald);
}
input:checked + .m26-slider:before {
  transform: translateX(14px);
}

/* Table Style */
.m26-table-wrap {
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  border: 1px solid var(--m26-border);
  border-radius: 14px;
  overflow-x: auto;
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.02);
}
.m26-table {
  width: 100%;
  min-width: 1050px;
  border-collapse: collapse;
  text-align: left;
  font-size: 13px;
  table-layout: fixed;
}
.m26-table th {
  background: #f8fafc;
  padding: 10px 12px;
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-muted);
  border-bottom: 1px solid var(--m26-border);
  white-space: nowrap;
}
.m26-table td {
  padding: 10px 12px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.04);
  color: var(--m26-text-secondary);
  vertical-align: middle;
  overflow: hidden;
  text-overflow: ellipsis;
}
.m26-table tr:hover td {
  background: #f1f5f9;
  color: var(--m26-text);
}
.m26-td-url {
  font-family: ui-monospace, SFMono-Regular, monospace;
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.m26-table-actions {
  display: inline-flex;
  align-items: center;
  justify-content: flex-end;
  gap: 5px;
  white-space: nowrap;
  flex-shrink: 0;
}

/* ==========================================================================
   Section 3: Models View (macOS 26 Glassmorphic Hub)
   ========================================================================== */
.m26-models-page-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-wrap: wrap;
  gap: 14px;
  margin-bottom: 6px;
}

.m26-models-title-area {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
}

.m26-family-pill-group {
  display: flex;
  align-items: center;
  gap: 3px;
  background: rgba(0, 0, 0, 0.04);
  padding: 3px;
  border-radius: 9px;
  border: 1px solid rgba(0, 0, 0, 0.05);
}

.m26-family-pill-btn {
  height: 26px;
  padding: 0 9px;
  font-size: 11.5px;
  font-weight: 500;
  color: var(--m26-text-secondary);
  background: transparent;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  transition: all 0.15s ease;
  user-select: none;
}

.m26-family-pill-btn:hover {
  color: var(--m26-text);
  background: rgba(255, 255, 255, 0.5);
}

.m26-family-pill-btn.active {
  color: var(--m26-text);
  background: #ffffff;
  font-weight: 650;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}

.m26-family-count {
  font-size: 10px;
  font-weight: 600;
  opacity: 0.65;
  background: rgba(0, 0, 0, 0.06);
  padding: 0 4px;
  border-radius: 4px;
  min-width: 14px;
  text-align: center;
}

.m26-family-pill-btn.active .m26-family-count {
  background: rgba(0, 0, 0, 0.08);
  opacity: 0.85;
}

.m26-models-actions-area {
  display: flex;
  align-items: center;
  gap: 10px;
}

/* Models Grid */
.m26-models-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(290px, 1fr));
  gap: 14px;
}

.m26-model-card {
  background: var(--m26-card-bg);
  border: 1px solid var(--m26-border);
  border-radius: 14px;
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  transition: all 0.18s cubic-bezier(0.16, 1, 0.3, 1);
  min-height: 200px;
  user-select: none;
}

.m26-model-card:hover {
  background: var(--m26-card-hover);
  border-color: rgba(79, 70, 229, 0.25);
  transform: translateY(-1.5px);
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.05);
}

/* Card Top: Brand Badge & Status Chip */
.m26-model-card-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-model-brand-badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2.5px 8px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 650;
  border: 1px solid transparent;
  letter-spacing: 0.01em;
}

.m26-model-brand-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  flex-shrink: 0;
}

.m26-model-brand-name {
  line-height: 1.2;
}

.m26-model-status-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border-radius: 9999px;
  font-size: 11px;
  font-weight: 600;
}

.m26-model-status-chip.healthy {
  background: rgba(5, 150, 105, 0.09);
  color: var(--m26-emerald);
  border: 1px solid rgba(5, 150, 105, 0.18);
}

.m26-model-status-chip.degraded {
  background: rgba(217, 119, 6, 0.09);
  color: var(--m26-amber);
  border: 1px solid rgba(217, 119, 6, 0.18);
}

.m26-model-status-chip.offline {
  background: rgba(225, 29, 72, 0.07);
  color: var(--m26-rose);
  border: 1px solid rgba(225, 29, 72, 0.15);
}

.m26-status-indicator-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  flex-shrink: 0;
}

.m26-status-indicator-dot.healthy {
  background: var(--m26-emerald);
  box-shadow: 0 0 5px rgba(5, 150, 105, 0.5);
}

.m26-status-indicator-dot.degraded {
  background: var(--m26-amber);
  box-shadow: 0 0 5px rgba(217, 119, 6, 0.5);
}

.m26-status-indicator-dot.offline {
  background: var(--m26-rose);
}

/* Main Info: Canonical Name + Copy Button */
.m26-model-main-info {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-model-title-box {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
  min-height: 36px;
}

.m26-model-canonical-name {
  font-size: 13px;
  font-weight: 700;
  color: var(--m26-text);
  line-height: 1.35;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-word;
  letter-spacing: -0.01em;
}

.m26-model-copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 1.5px 6px;
  font-size: 10.5px;
  font-weight: 500;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.03);
  border: 1px solid var(--m26-border);
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;
  margin-top: 1px;
}

.m26-model-copy-btn:hover {
  color: var(--m26-accent);
  border-color: rgba(79, 70, 229, 0.25);
  background: rgba(79, 70, 229, 0.05);
}

/* Compact & Clickable Aliases Row */
.m26-model-aliases-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 7px;
  background: rgba(99, 102, 241, 0.04);
  border: 1px solid rgba(99, 102, 241, 0.1);
  border-radius: 6px;
  font-size: 10.5px;
  overflow: hidden;
  height: 24px;
  box-sizing: border-box;
  transition: all 0.15s ease;
}

.m26-model-aliases-bar.clickable {
  cursor: pointer;
  user-select: none;
}

.m26-model-aliases-bar.clickable:hover {
  background: rgba(99, 102, 241, 0.09);
  border-color: rgba(99, 102, 241, 0.28);
  transform: translateY(-0.5px);
  box-shadow: 0 2px 5px rgba(99, 102, 241, 0.08);
}

.m26-model-aliases-bar.direct {
  background: rgba(0, 0, 0, 0.02);
  border-color: rgba(0, 0, 0, 0.05);
}

.m26-model-aliases-bar.direct.clickable:hover {
  background: rgba(0, 0, 0, 0.05);
  border-color: rgba(0, 0, 0, 0.12);
  box-shadow: 0 2px 5px rgba(0, 0, 0, 0.04);
}

.m26-aliases-tag {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  font-weight: 600;
  color: #6366f1;
  white-space: nowrap;
  flex-shrink: 0;
}

.m26-aliases-tag.direct {
  color: var(--m26-text-muted);
}

.m26-aliases-text {
  color: var(--m26-text-muted);
  font-size: 10px;
  opacity: 0.85;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}

.m26-aliases-more-btn {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 1px;
  font-size: 9.5px;
  font-weight: 500;
  color: #6366f1;
  opacity: 0.65;
  flex-shrink: 0;
  transition: opacity 0.15s ease;
}

.m26-model-aliases-bar.clickable:hover .m26-aliases-more-btn {
  opacity: 1;
}

.m26-model-aliases-bar.direct .m26-aliases-more-btn {
  color: var(--m26-text-muted);
}

/* Model Aliases Modal Styles */
.m26-aliases-modal-sheet {
  max-width: 680px;
  width: 92%;
}

.m26-alias-modal-body {
  padding: 18px 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-height: calc(85vh - 160px);
  overflow-y: auto;
  box-sizing: border-box;
}

.m26-alias-guide-card {
  padding: 10px 14px;
  border-radius: 10px;
  background: rgba(99, 102, 241, 0.04);
  border: 1px solid rgba(99, 102, 241, 0.12);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-guide-badge {
  font-size: 10px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 4px;
  background: #6366f1;
  color: #ffffff;
}

.m26-alias-guide-code {
  font-size: 12px;
  background: rgba(255, 255, 255, 0.9);
  border: 1px solid rgba(15, 23, 42, 0.08);
  padding: 6px 10px;
  border-radius: 6px;
}

.m26-alias-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.m26-alias-section-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
}

.m26-alias-section-title {
  font-size: 12px;
  font-weight: 700;
  color: var(--m26-text);
}

.m26-alias-items-grid {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.m26-alias-item-card {
  padding: 10px 12px;
  border-radius: 10px;
  background: #f8fafc;
  border: 1px solid rgba(15, 23, 42, 0.06);
  display: flex;
  flex-direction: column;
  gap: 8px;
  transition: border-color 0.15s ease;
}

.m26-alias-item-card:hover {
  border-color: rgba(99, 102, 241, 0.2);
}

.m26-alias-card-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.m26-alias-name {
  font-size: 12px;
  font-weight: 600;
  color: #1e293b;
  word-break: break-all;
}

.m26-alias-copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2.5px 8px;
  font-size: 11px;
  font-weight: 500;
  border-radius: 6px;
  border: 1px solid rgba(15, 23, 42, 0.1);
  background: #ffffff;
  color: var(--m26-text-secondary);
  cursor: pointer;
  flex-shrink: 0;
  transition: all 0.15s ease;
}

.m26-alias-copy-btn:hover {
  border-color: #6366f1;
  color: #6366f1;
  background: rgba(99, 102, 241, 0.04);
}

.m26-model-classify-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 26px;
  padding: 0 9px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 550;
  background: rgba(0, 0, 0, 0.04);
  color: var(--m26-text-secondary);
  border: 1px solid rgba(0, 0, 0, 0.08);
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-model-classify-btn:hover {
  background: rgba(79, 70, 229, 0.08);
  color: var(--m26-accent);
  border-color: rgba(79, 70, 229, 0.25);
}

.m26-custom-mapping-badge {
  display: inline-flex;
  align-items: center;
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 10.5px;
  font-weight: 600;
  background: rgba(79, 70, 229, 0.08);
  color: #4f46e5;
  border: 1px solid rgba(79, 70, 229, 0.2);
}

.m26-alias-unlink-btn {
  display: inline-flex;
  align-items: center;
  padding: 2.5px 8px;
  font-size: 11px;
  font-weight: 500;
  border-radius: 6px;
  border: 1px solid rgba(225, 29, 72, 0.2);
  background: rgba(225, 29, 72, 0.04);
  color: #e11d48;
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-alias-unlink-btn:hover {
  background: #e11d48;
  color: #ffffff;
  border-color: #e11d48;
}

.m26-badge-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: 9999px;
  font-size: 10px;
  font-weight: 700;
  background: var(--m26-accent);
  color: #ffffff;
  margin-left: 2px;
}

.m26-custom-mappings-table {
  border: 1px solid rgba(15, 23, 42, 0.08);
  border-radius: 8px;
  background: #ffffff;
  overflow: hidden;
}

.m26-mapping-row {
  display: flex;
  align-items: center;
  padding: 8px 12px;
  border-bottom: 1px solid rgba(15, 23, 42, 0.05);
  font-size: 12px;
}

.m26-mapping-row:last-child {
  border-bottom: none;
}

.m26-mapping-row.header {
  background: rgba(15, 23, 42, 0.02);
  font-size: 11px;
  font-weight: 600;
  color: #64748b;
}

.m26-mapping-row .col-source {
  flex: 1;
  min-width: 0;
  color: #1e293b;
  word-break: break-all;
}

.m26-mapping-row .col-arrow {
  padding: 0 12px;
  color: #94a3b8;
  flex-shrink: 0;
}

.m26-mapping-row .col-target {
  flex: 1;
  min-width: 0;
  word-break: break-all;
}

.m26-mapping-row .col-action {
  flex-shrink: 0;
  margin-left: 12px;
}

.m26-alias-providers-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  font-size: 11px;
}

.m26-alias-prov-label {
  color: var(--m26-text-muted);
  font-size: 11px;
  flex-shrink: 0;
}

.m26-alias-prov-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
}

.m26-alias-prov-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 7px;
  border-radius: 5px;
  background: #ffffff;
  border: 1px solid rgba(15, 23, 42, 0.08);
  font-size: 11px;
  color: #334155;
}

.ch-lat-num {
  font-size: 10px;
  font-family: monospace;
  color: #94a3b8;
}

.m26-mapping-table-wrapper {
  border: 1px solid rgba(15, 23, 42, 0.08);
  border-radius: 10px;
  overflow: hidden;
  background: #ffffff;
}

.m26-mapping-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 11px;
  text-align: left;
}

.m26-mapping-table th {
  background: #f8fafc;
  padding: 8px 14px;
  font-weight: 600;
  color: var(--m26-text-secondary);
  border-bottom: 1px solid rgba(15, 23, 42, 0.08);
  white-space: nowrap;
}

.m26-mapping-table td {
  padding: 9px 14px;
  border-bottom: 1px solid rgba(15, 23, 42, 0.04);
  white-space: nowrap;
}

.m26-mapping-table tr:last-child td {
  border-bottom: none;
}

.m26-table-copy-icon {
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 2px;
  color: #94a3b8;
  display: inline-flex;
  align-items: center;
  border-radius: 3px;
  transition: color 0.15s ease;
}

.m26-table-copy-icon:hover {
  color: #6366f1;
}

.m26-mapping-chip {
  display: inline-flex;
  align-items: center;
  padding: 2px 7px;
  border-radius: 4px;
  font-size: 10.5px;
  font-weight: 500;
  white-space: nowrap;
}

.m26-mapping-chip.rewrite {
  background: rgba(99, 102, 241, 0.08);
  color: #4f46e5;
  border: 1px solid rgba(99, 102, 241, 0.15);
}

.m26-mapping-chip.direct {
  background: rgba(16, 185, 129, 0.08);
  color: #059669;
  border: 1px solid rgba(16, 185, 129, 0.15);
}

.m26-empty-alias-tip {
  padding: 18px;
  text-align: center;
  color: var(--m26-text-muted);
  background: #f8fafc;
  border-radius: 8px;
  border: 1px dashed rgba(15, 23, 42, 0.1);
}

.m26-aliases-modal-sheet .m26-modal-header {
  padding: 16px 20px;
}

.m26-aliases-modal-sheet .m26-close-btn {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s ease;
  font-size: 14px;
}

.m26-aliases-modal-sheet .m26-close-btn:hover {
  background: rgba(0, 0, 0, 0.06);
  color: var(--m26-text);
}

.m26-aliases-modal-sheet .m26-modal-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 20px;
}

.m26-aliases-modal-sheet .m26-modal-tip {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  font-size: 11px;
  line-height: 1.4;
  color: var(--m26-text-muted);
}

.m26-aliases-modal-sheet .m26-modal-tip svg {
  flex-shrink: 0;
}

.m26-aliases-modal-sheet .m26-modal-footer .m26-action-btn {
  white-space: nowrap;
  flex-shrink: 0;
  padding: 6px 18px;
  font-size: 12px;
  min-width: 64px;
  text-align: center;
}

/* Model Multi-Provider Speed Test Modal Styles */
.m26-speedtest-modal-sheet {
  max-width: 800px;
  width: 94%;
}

.m26-speedtest-modal-sheet .m26-modal-header {
  padding: 18px 22px;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.m26-speedtest-modal-sheet .m26-close-btn {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s ease;
  font-size: 14px;
  flex-shrink: 0;
  margin-top: 2px;
}

.m26-speedtest-modal-sheet .m26-close-btn:hover {
  background: rgba(0, 0, 0, 0.06);
  color: var(--m26-text);
}

.m26-speedtest-body {
  padding: 16px 22px;
  max-height: calc(85vh - 160px);
  overflow-y: auto;
}

.m26-tag-preferred {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 600;
  color: #059669;
  background: rgba(16, 185, 129, 0.12);
  border: 1px solid rgba(16, 185, 129, 0.28);
  box-shadow: 0 0 8px rgba(16, 185, 129, 0.12);
}

.dark .m26-tag-preferred {
  color: #34d399;
  background: rgba(16, 185, 129, 0.18);
  border-color: rgba(52, 211, 153, 0.35);
}

.m26-tag-disabled {
  display: inline-flex;
  align-items: center;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 11px;
  color: #94a3b8;
  background: rgba(148, 163, 184, 0.14);
}

.m26-fastest-provider-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 500;
  color: #047857;
  background: rgba(16, 185, 129, 0.08);
  border: 1px solid rgba(16, 185, 129, 0.22);
}

.dark .m26-fastest-provider-pill {
  color: #6ee7b7;
  background: rgba(16, 185, 129, 0.16);
  border-color: rgba(16, 185, 129, 0.32);
}

.m26-fastest-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #10b981;
  box-shadow: 0 0 6px #10b981;
}

.m26-speedtest-alias-tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: rgba(99, 102, 241, 0.06);
  border: 1px solid rgba(99, 102, 241, 0.15);
  border-radius: 4px;
  padding: 1px 6px;
  font-size: 11px;
}

.dark .m26-speedtest-alias-tag {
  background: rgba(99, 102, 241, 0.15);
  border-color: rgba(99, 102, 241, 0.3);
}

.m26-model-item-card.is-preferred {
  border-color: rgba(16, 185, 129, 0.45);
  background: rgba(16, 185, 129, 0.03);
}

.dark .m26-model-item-card.is-preferred {
  border-color: rgba(52, 211, 153, 0.45);
  background: rgba(16, 185, 129, 0.06);
}

/* Channels Pill Box */
.m26-model-channels-box {
  display: flex;
  flex-direction: column;
  gap: 5px;
  margin-top: 2px;
}

.m26-channels-meta-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-channels-count {
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-secondary);
}

.m26-channels-pill-list {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
  min-height: 24px;
}

.m26-channel-mini-pill {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 7px;
  border-radius: 5px;
  font-size: 10.5px;
  font-weight: 500;
  background: rgba(0, 0, 0, 0.04);
  color: var(--m26-text);
  border: 1px solid rgba(0, 0, 0, 0.05);
  transition: all 0.12s ease;
  cursor: default;
}

.m26-channel-mini-pill.healthy {
  background: rgba(5, 150, 105, 0.06);
  color: #065f46;
  border-color: rgba(5, 150, 105, 0.12);
}

.m26-channel-mini-pill.degraded {
  background: rgba(217, 119, 6, 0.06);
  color: #92400e;
  border-color: rgba(217, 119, 6, 0.12);
}

.m26-channel-mini-pill.offline {
  background: rgba(225, 29, 72, 0.05);
  color: #9f1239;
  border-color: rgba(225, 29, 72, 0.1);
}

.m26-channel-mini-pill.more {
  color: var(--m26-text-muted);
  font-size: 10px;
  background: rgba(0, 0, 0, 0.03);
}

/* Card Footer Action */
.m26-model-card-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  margin-top: auto;
  padding-top: 8px;
  border-top: 1px solid rgba(0, 0, 0, 0.04);
}

.m26-model-test-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 26px;
  padding: 0 10px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 550;
  background: rgba(79, 70, 229, 0.08);
  color: var(--m26-accent);
  border: 1px solid rgba(79, 70, 229, 0.18);
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-model-test-btn:hover {
  background: var(--m26-accent);
  color: #ffffff;
  border-color: var(--m26-accent);
  box-shadow: 0 2px 6px rgba(79, 70, 229, 0.25);
}

/* ==========================================================================
   Section 4: Logs View
   ========================================================================== */
.m26-logs-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.m26-logs-count {
  font-size: 13px;
  font-weight: 600;
  color: var(--m26-text-secondary);
}
.m26-logs-right {
  display: flex;
  gap: 8px;
}
.m26-status-pill {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  white-space: nowrap;
  padding: 2px 7px;
  border-radius: 5px;
  font-weight: 600;
  font-size: 11px;
  line-height: 1.2;
}
.m26-status-pill.status-200 { background: rgba(5, 150, 105, 0.12); color: var(--m26-emerald); }
.m26-status-pill.status-429 { background: rgba(217, 119, 6, 0.12); color: var(--m26-amber); }
.m26-status-pill.status-err { background: rgba(225, 29, 72, 0.12); color: var(--m26-rose); }

.m26-model-badge {
  display: inline-flex;
  align-items: center;
  width: fit-content;
  max-width: 100%;
  gap: 5px;
  padding: 2px 8px;
  border-radius: 9999px;
  background: #fff7ed;
  border: 1px solid #fed7aa;
  color: #c2410c;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.2;
  white-space: nowrap;
}
.m26-model-badge-icon {
  flex-shrink: 0;
  color: #ea580c;
}
.m26-pill-total {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 2px 7px;
  border-radius: 9999px;
  background: #dcfce7;
  color: #15803d;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.2;
  white-space: nowrap;
}
.m26-pill-ttft {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 2px 7px;
  border-radius: 9999px;
  background: #ffe4e6;
  color: #e11d48;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.2;
  white-space: nowrap;
}
.m26-pill-stream {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 1px 5px;
  border-radius: 9999px;
  background: #dbeafe;
  color: #2563eb;
  font-size: 10px;
  font-weight: 600;
  line-height: 1.2;
  white-space: nowrap;
}
.m26-token-pulse-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #22c55e;
  animation: m26-token-pulse 1.2s infinite ease-in-out;
  flex-shrink: 0;
}
@keyframes m26-token-pulse {
  0% { transform: scale(0.8); opacity: 0.6; }
  50% { transform: scale(1.3); opacity: 1; }
  100% { transform: scale(0.8); opacity: 0.6; }
}

/* ==========================================================================
   Section 5: Settings View (macOS 26 Glassmorphic Design)
   ========================================================================== */
.m26-settings-container {
  display: flex;
  flex-direction: column;
  gap: 22px;
  width: 100%;
  max-width: 980px;
  margin: 0 auto;
  padding: 0 0 44px;
}

.m26-pref-group {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.m26-pref-group-header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 4px;
}

.m26-pref-group-icon {
  width: 32px;
  height: 32px;
  border-radius: 9px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.m26-pref-group-icon.icon-route {
  background: rgba(99, 102, 241, 0.1);
  color: #6366f1;
}

.m26-pref-group-icon.icon-auth {
  background: rgba(14, 165, 233, 0.1);
  color: #0284c7;
}

.m26-pref-group-icon.icon-storage {
  background: rgba(16, 185, 129, 0.1);
  color: #059669;
}

.m26-pref-group-titles {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.m26-pref-title {
  font-size: 14px;
  font-weight: 650;
  color: var(--m26-text);
  letter-spacing: -0.01em;
  margin: 0;
  line-height: 1.25;
}

.m26-pref-subtitle {
  font-size: 11.5px;
  color: var(--m26-text-muted);
  line-height: 1.35;
}

/* Route strategy selection cards */
.m26-route-cards {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 14px;
}

.m26-route-card {
  background: var(--m26-card-bg);
  border: 1px solid var(--m26-border);
  border-radius: 14px;
  padding: 13px 16px;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 9px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  transition: all 0.18s cubic-bezier(0.16, 1, 0.3, 1);
  user-select: none;
}

.m26-route-card:hover {
  background: var(--m26-card-hover);
  border-color: rgba(0, 0, 0, 0.14);
  transform: translateY(-1.5px);
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.05);
}

.m26-route-card.selected {
  border-color: var(--m26-accent);
  background: rgba(79, 70, 229, 0.04);
  box-shadow: 0 0 0 1.5px var(--m26-accent), 0 4px 16px rgba(79, 70, 229, 0.08);
}

.m26-route-card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-route-type-badge {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.m26-route-type-badge.smart_quality,
.m26-route-type-badge.priority {
  background: rgba(59, 130, 246, 0.12);
  color: #2563eb;
}

.m26-route-type-badge.latency {
  background: rgba(16, 185, 129, 0.1);
  color: #059669;
}

.m26-route-type-badge.roundrobin {
  background: rgba(99, 102, 241, 0.1);
  color: #6366f1;
}

.m26-route-pill-rec {
  font-size: 10px;
  font-weight: 700;
  color: #059669;
  background: rgba(16, 185, 129, 0.12);
  padding: 2px 7px;
  border-radius: 9999px;
  letter-spacing: 0.02em;
}

.m26-route-check-indicator {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 1.5px solid var(--m26-border-bright);
  display: flex;
  align-items: center;
  justify-content: center;
  color: #ffffff;
  background: transparent;
  transition: all 0.15s ease;
}

.m26-route-card.selected .m26-route-check-indicator {
  background: var(--m26-accent);
  border-color: var(--m26-accent);
}

.m26-route-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.m26-route-name-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.m26-route-name {
  font-size: 13px;
  font-weight: 650;
  color: var(--m26-text);
}

.m26-route-code-tag {
  font-size: 10px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.04);
  padding: 1px 4px;
  border-radius: 3px;
}

.m26-route-desc {
  font-size: 11px;
  color: var(--m26-text-muted);
  line-height: 1.45;
  margin: 0;
  min-height: 28px;
}

/* Preference card container and rows */
.m26-pref-card {
  background: var(--m26-card-bg);
  border: 1px solid var(--m26-border);
  border-radius: 14px;
  display: flex;
  flex-direction: column;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  overflow: hidden;
}

.m26-pref-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 18px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.05);
  gap: 16px;
  transition: background 0.12s ease;
}

.m26-pref-row:hover {
  background: rgba(0, 0, 0, 0.01);
}

.m26-pref-row:last-child {
  border-bottom: none;
}

.m26-pref-meta {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.m26-pref-meta .label {
  font-size: 13.5px;
  font-weight: 600;
  color: var(--m26-text);
}

.m26-pref-meta .desc {
  font-size: 12px;
  color: var(--m26-text-muted);
  line-height: 1.45;
}

.m26-pref-ctrl {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

/* Credential field (URL & Key) */
.m26-cred-field {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 6px 4px 8px;
  background: rgba(248, 250, 252, 0.95);
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 8px;
  transition: all 0.15s ease;
}

.m26-cred-field:hover {
  background: #ffffff;
  border-color: rgba(0, 0, 0, 0.16);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.03);
}

.m26-cred-field-tag {
  font-size: 10px;
  font-weight: 700;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.05);
  padding: 2px 5px;
  border-radius: 4px;
  letter-spacing: 0.5px;
  user-select: none;
  flex-shrink: 0;
}

.m26-cred-field-val {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
  color: var(--m26-text);
  max-width: 280px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
  user-select: text;
  -webkit-user-select: text;
  transition: color 0.15s ease;
}

.m26-cred-field-val:hover {
  color: var(--m26-accent);
}

.m26-cred-field-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.m26-cred-field-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 500;
  color: var(--m26-text-secondary);
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.09);
  border-radius: 5px;
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;
}

.m26-cred-field-btn:hover {
  background: #f8fafc;
  color: var(--m26-text);
  border-color: rgba(0, 0, 0, 0.18);
}

/* Custom styled Select for macOS */
.m26-select {
  height: 32px;
  padding: 0 30px 0 12px;
  font-size: 12.5px;
  font-weight: 500;
  color: var(--m26-text);
  background-color: #ffffff;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2364748b' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 9px center;
  background-size: 11px;
  border: 1px solid var(--m26-border-bright);
  border-radius: 8px;
  outline: none;
  cursor: pointer;
  appearance: none;
  -webkit-appearance: none;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: all 0.15s ease;
}

.m26-select:hover {
  border-color: rgba(0, 0, 0, 0.2);
  background-color: #fafafa;
}

.m26-select:focus {
  border-color: var(--m26-accent);
  box-shadow: 0 0 0 2px rgba(79, 70, 229, 0.15);
}

.m26-db-chip {
  font-size: 11px;
  font-weight: 500;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.04);
  padding: 1.5px 6px;
  border-radius: 4px;
}

.m26-btn-separator {
  width: 1px;
  height: 18px;
  background: rgba(0, 0, 0, 0.1);
  margin: 0 4px;
}

/* ==========================================================================
   Modals: macOS Sheet Dialog Style (Light Mode Pure White Glass)
   ========================================================================== */
.m26-modal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 999;
  background: rgba(15, 23, 42, 0.3);
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  user-select: none;
}
.m26-modal-sheet {
  width: 100%;
  max-width: 520px;
  background: #ffffff;
  border: 1px solid var(--m26-border-bright);
  border-radius: 18px;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.16), 0 4px 16px rgba(0, 0, 0, 0.06);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  animation: sheetIn 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}
@keyframes sheetIn {
  from { opacity: 0; transform: scale(0.96) translateY(-10px); }
  to { opacity: 1; transform: scale(1) translateY(0); }
}

.m26-modal-header {
  padding: 16px 20px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid var(--m26-border);
  user-select: none;
  cursor: default;
}
.m26-modal-title {
  font-size: 15px;
  font-weight: 700;
  color: var(--m26-text);
}
.m26-close-btn {
  background: transparent;
  border: none;
  color: var(--m26-text-muted);
  font-size: 16px;
  cursor: pointer;
}
.m26-close-btn:hover {
  color: var(--m26-text);
}

.m26-modal-body {
  padding: 22px 24px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  max-height: calc(85vh - 120px);
  overflow-y: auto;
  box-sizing: border-box;
}
.m26-form-group {
  display: flex;
  flex-direction: column;
  gap: 7px;
  width: 100%;
  flex-shrink: 0;
  box-sizing: border-box;
}
.m26-form-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 14px;
  width: 100%;
  box-sizing: border-box;
}
.m26-label {
  display: block;
  font-size: 12px;
  font-weight: 600;
  color: var(--m26-text-secondary);
  line-height: 1.2;
}
.m26-input, .m26-select, .m26-textarea {
  box-sizing: border-box;
  background: #f8fafc;
  border: 1px solid var(--m26-border-bright);
  border-radius: 8px;
  padding: 0 12px;
  height: 38px;
  color: var(--m26-text);
  font-size: 13px;
  outline: none;
  transition: all 0.15s ease;
  user-select: text;
  -webkit-user-select: text;
}
.m26-form-group .m26-input, .m26-form-group .m26-select, .m26-form-group .m26-textarea {
  width: 100%;
}
.m26-input:focus, .m26-select:focus, .m26-textarea:focus {
  border-color: var(--m26-accent);
  background: #ffffff;
  box-shadow: 0 0 0 3px var(--m26-accent-glow);
}
.m26-select {
  appearance: auto;
  -webkit-appearance: menulist;
  cursor: pointer;
  line-height: 36px;
}
.m26-select option {
  background: #ffffff;
  color: var(--m26-text);
}
.m26-textarea {
  height: auto;
  min-height: 84px;
  padding: 8px 12px;
  line-height: 1.5;
  resize: vertical;
}
.m26-text-btn {
  background: transparent;
  border: none;
  color: var(--m26-accent);
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
}
.m26-text-btn:hover {
  text-decoration: underline;
}

.m26-modal-footer {
  padding: 14px 20px;
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  background: #f8fafc;
  border-top: 1px solid var(--m26-border);
}

/* Floating Toast */
.m26-toast-pill {
  position: fixed;
  bottom: 28px;
  left: 50%;
  transform: translateX(-50%);
  background: rgba(15, 23, 42, 0.9);
  border: 1px solid rgba(255, 255, 255, 0.15);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.2);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  padding: 9px 18px;
  border-radius: 99px;
  display: flex;
  align-items: center;
  gap: 9px;
  z-index: 1000;
}
.m26-toast-beacon {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #818cf8;
  box-shadow: 0 0 8px #818cf8;
}
.m26-toast-text {
  font-size: 13px;
  font-weight: 500;
  color: #fff;
}
.m26-toast-enter-active, .m26-toast-leave-active {
  transition: all 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}
.m26-toast-enter-from, .m26-toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 15px);
}

.m26-empty-state {
  padding: 36px 0;
  text-align: center;
  color: var(--m26-text-muted);
  font-size: 13px;
}

/* ==========================================================================
   Desktop Native Spacer & Clickable Models Strip
   ========================================================================== */
.m26-native-titlebar-spacer {
  height: 16px;
  width: 100%;
}

.m26-ch-models-strip.clickable {
  cursor: pointer;
  padding: 3px 6px;
  margin: -3px -6px;
  border-radius: 6px;
  transition: all 0.15s ease;
  user-select: none;
}
.m26-ch-models-strip.clickable:hover {
  background: rgba(79, 70, 229, 0.08);
}
.m26-ch-models-strip.clickable:hover .m26-model-badge.count {
  background: var(--m26-accent);
  color: #ffffff;
}

/* Spin loader dot */
.m26-spin-dot {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 1.5px solid currentColor;
  border-top-color: transparent;
  border-radius: 50%;
  animation: m26Spin 0.7s linear infinite;
}
.m26-spin-dot.mini {
  width: 8px;
  height: 8px;
  border-width: 1.2px;
}
@keyframes m26Spin {
  to { transform: rotate(360deg); }
}

/* Live polling indicator & active button */
.m26-btn-active-green {
  background: rgba(16, 185, 129, 0.12) !important;
  color: #059669 !important;
  border-color: rgba(16, 185, 129, 0.35) !important;
}
.m26-btn-active-green:hover {
  background: rgba(16, 185, 129, 0.18) !important;
}
.m26-live-indicator {
  display: inline-block;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background-color: #10b981;
  box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.7);
  animation: m26LivePulse 1.8s infinite cubic-bezier(0.66, 0, 0, 1);
  margin-right: 2px;
}
@keyframes m26LivePulse {
  0% {
    box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.7);
  }
  70% {
    box-shadow: 0 0 0 5px rgba(16, 185, 129, 0);
  }
  100% {
    box-shadow: 0 0 0 0 rgba(16, 185, 129, 0);
  }
}
.m26-pause-indicator {
  display: inline-block;
  font-size: 10px;
  opacity: 0.7;
  margin-right: 2px;
}

/* ==========================================================================
   Confirm Dialog Sheet
   ========================================================================== */
.m26-confirm-sheet {
  max-width: 440px;
}
.m26-confirm-content {
  display: flex;
  gap: 16px;
  align-items: flex-start;
}
.m26-confirm-icon-wrap {
  width: 44px;
  height: 44px;
  border-radius: 12px;
  background: rgba(79, 70, 229, 0.1);
  color: var(--m26-accent);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.m26-confirm-icon-wrap.danger {
  background: rgba(225, 29, 72, 0.1);
  color: var(--m26-rose);
}
.m26-confirm-texts {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.m26-confirm-main {
  font-size: 14px;
  font-weight: 600;
  color: var(--m26-text);
  line-height: 1.4;
  margin: 0;
}
.m26-confirm-desc {
  font-size: 12px;
  color: var(--m26-text-muted);
  line-height: 1.5;
  margin: 0;
}
.m26-action-btn.danger {
  background: #e11d48;
  color: #ffffff;
  box-shadow: 0 2px 8px rgba(225, 29, 72, 0.28);
}
.m26-action-btn.danger:hover {
  background: #be123c;
  box-shadow: 0 4px 12px rgba(225, 29, 72, 0.35);
}
.m26-action-btn.danger:active {
  transform: translateY(1px);
}
.m26-action-btn.danger-subtle {
  background: rgba(225, 29, 72, 0.08);
  color: #e11d48;
  border: 1px solid rgba(225, 29, 72, 0.2);
  box-shadow: none;
}
.m26-action-btn.danger-subtle:hover {
  background: rgba(225, 29, 72, 0.14);
  border-color: rgba(225, 29, 72, 0.35);
  color: #be123c;
}
.m26-confirm-backdrop {
  z-index: 1200;
}

/* ==========================================================================
   Model Latency & Speed Test Sheet
   ========================================================================== */
.m26-models-sheet {
  max-width: 700px;
  max-height: 85vh;
}
.m26-models-sheet .m26-modal-footer {
  justify-content: flex-start;
  padding: 12px 20px;
}
.m26-models-sheet .m26-modal-hint {
  line-height: 1.5;
  color: var(--m26-text-muted);
}
.m26-modal-subtitle {
  font-size: 12px;
  color: var(--m26-text-muted);
  margin-top: 2px;
}
.m26-modal-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 20px;
  background: #f8fafc;
  border-bottom: 1px solid var(--m26-border);
}
.m26-modal-search {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  height: 32px;
  flex: 1;
  max-width: 300px;
  background: #ffffff;
  border: 1px solid var(--m26-border-bright);
  border-radius: 8px;
  color: var(--m26-text-muted);
  transition: all 0.15s ease;
}
.m26-modal-search:focus-within {
  border-color: var(--m26-accent);
  box-shadow: 0 0 0 3px rgba(79, 70, 229, 0.12);
  color: var(--m26-text);
}
.m26-modal-search-input {
  border: none;
  background: transparent;
  outline: none;
  font-size: 12px;
  color: var(--m26-text);
  width: 100%;
}
.m26-model-list-body {
  padding: 16px 20px;
  max-height: 420px;
  overflow-y: auto;
}
.m26-model-grid {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.m26-model-item-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 14px;
  background: #ffffff;
  border: 1px solid var(--m26-border);
  border-radius: 10px;
  transition: all 0.15s ease;
}
.m26-model-item-card:hover {
  background: #f8fafc;
  border-color: rgba(79, 70, 229, 0.25);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.04);
}
.m26-model-item-info {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
  flex: 1;
}
.m26-model-idx {
  font-size: 11px;
  font-family: ui-monospace, monospace;
  color: var(--m26-text-muted);
  font-weight: 600;
}
.m26-model-full-name {
  font-size: 13px;
  font-weight: 600;
  font-family: ui-monospace, monospace;
  color: var(--m26-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 320px;
}
.m26-tag-primary {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: rgba(79, 70, 229, 0.1);
  color: var(--m26-accent);
  font-weight: 600;
  white-space: nowrap;
}
.m26-model-ch-url {
  font-size: 11px;
  color: var(--m26-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 380px;
}
.m26-model-item-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}
.m26-model-lat-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 8px;
  border-radius: 99px;
  font-size: 11px;
  font-weight: 600;
  font-family: ui-monospace, monospace;
  background: rgba(100, 116, 139, 0.08);
  color: #64748b;
  border: 1px solid rgba(100, 116, 139, 0.15);
  transition: all 0.15s ease;
}
.m26-model-lat-pill .lat-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}
.m26-model-lat-pill.fast {
  background: rgba(22, 163, 74, 0.1);
  color: #16a34a;
  border-color: rgba(22, 163, 74, 0.25);
}
.m26-model-lat-pill.normal {
  background: rgba(2, 132, 199, 0.1);
  color: #0284c7;
  border-color: rgba(2, 132, 199, 0.25);
}
.m26-model-lat-pill.slow {
  background: rgba(217, 119, 6, 0.1);
  color: #d97706;
  border-color: rgba(217, 119, 6, 0.25);
}
.m26-model-lat-pill.timeout {
  background: rgba(225, 29, 72, 0.1);
  color: #e11d48;
  border-color: rgba(225, 29, 72, 0.25);
}
.m26-model-lat-pill.probing {
  background: rgba(79, 70, 229, 0.1);
  color: var(--m26-accent);
  border-color: rgba(79, 70, 229, 0.25);
}
.m26-model-lat-pill.idle {
  background: rgba(100, 116, 139, 0.06);
  color: #94a3b8;
  border-color: rgba(100, 116, 139, 0.12);
}
.m26-model-lat-pill.has-err-link {
  cursor: pointer;
}
.m26-model-lat-pill.has-err-link:hover {
  filter: brightness(0.95);
  box-shadow: 0 1px 4px rgba(225, 29, 72, 0.2);
}

/* ==========================================================================
   Error Banner & Diagnostic Modal Sheet
   ========================================================================== */
.m26-ch-error-strip {
  margin-top: 8px;
  padding: 6px 10px;
  background: rgba(225, 29, 72, 0.06);
  border: 1px solid rgba(225, 29, 72, 0.18);
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-ch-error-strip:hover {
  background: rgba(225, 29, 72, 0.1);
  border-color: rgba(225, 29, 72, 0.32);
}
.m26-ch-error-strip-left {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex: 1;
  color: var(--m26-rose);
}
.error-strip-icon {
  flex-shrink: 0;
}
.m26-ch-error-strip-msg {
  font-size: 11px;
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.m26-ch-error-strip-link {
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-rose);
  flex-shrink: 0;
}
.m26-status-err-tag {
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 4px;
  background: rgba(225, 29, 72, 0.12);
  color: var(--m26-rose);
  font-weight: 600;
  margin-left: 4px;
  white-space: nowrap;
  flex-shrink: 0;
}
.m26-ch-status-box.has-error {
  cursor: pointer;
  padding: 2px 6px;
  margin: -2px -6px;
  border-radius: 6px;
  transition: all 0.15s ease;
}
.m26-ch-status-box.has-error:hover {
  background: rgba(225, 29, 72, 0.08);
}
.m26-btn-micro.danger-soft {
  background: rgba(225, 29, 72, 0.08);
  border-color: rgba(225, 29, 72, 0.2);
  color: var(--m26-rose);
}
.m26-btn-micro.danger-soft:hover {
  background: rgba(225, 29, 72, 0.15);
  border-color: rgba(225, 29, 72, 0.35);
}

/* Diagnostic Modal Sheet */
.m26-diagnostic-sheet {
  max-width: 640px;
  max-height: 88vh;
}
.m26-diag-icon-badge {
  width: 36px;
  height: 36px;
  border-radius: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.m26-diag-icon-badge.danger {
  background: rgba(225, 29, 72, 0.1);
  color: var(--m26-rose);
}
.m26-diagnostic-body {
  padding: 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-height: calc(88vh - 130px);
  overflow-y: auto;
}
.m26-diag-card {
  padding: 14px 16px;
  border-radius: 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.m26-diag-card.danger {
  background: rgba(225, 29, 72, 0.05);
  border: 1px solid rgba(225, 29, 72, 0.2);
}
.m26-diag-card.advice {
  background: #f8fafc;
  border: 1px solid var(--m26-border);
}
.m26-diag-card-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  font-weight: 700;
  color: var(--m26-text);
}
.m26-diag-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
}
.m26-diag-dot.danger {
  background: var(--m26-rose);
  box-shadow: 0 0 6px rgba(225, 29, 72, 0.5);
}
.m26-diag-dot.info {
  background: var(--m26-accent);
  box-shadow: 0 0 6px rgba(79, 70, 229, 0.5);
}
.m26-diag-code-pill {
  margin-left: auto;
  font-family: ui-monospace, monospace;
  font-size: 11px;
  padding: 1px 7px;
  border-radius: 4px;
  background: rgba(225, 29, 72, 0.12);
  color: var(--m26-rose);
  font-weight: 700;
}
.m26-diag-main-msg {
  font-size: 13px;
  font-weight: 600;
  color: #1e293b;
  line-height: 1.5;
}
.m26-diag-meta-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 10px;
}
.m26-diag-meta-item {
  padding: 10px 12px;
  background: #f8fafc;
  border: 1px solid var(--m26-border);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.m26-diag-meta-item .label {
  font-size: 11px;
  color: var(--m26-text-muted);
}
.m26-diag-meta-item .value {
  font-size: 12px;
  font-weight: 600;
  color: var(--m26-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.m26-diag-advice-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.m26-diag-tip-item {
  display: flex;
  gap: 10px;
  align-items: flex-start;
  font-size: 12px;
  line-height: 1.5;
  color: #334155;
}
.m26-diag-tip-item .tip-num {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: rgba(79, 70, 229, 0.1);
  color: var(--m26-accent);
  font-weight: 700;
  font-size: 11px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  margin-top: 1px;
}
.m26-diag-tip-item code {
  font-family: ui-monospace, monospace;
  background: rgba(0, 0, 0, 0.05);
  padding: 1px 4px;
  border-radius: 4px;
  font-size: 11px;
  color: var(--m26-accent);
}
.m26-diag-raw-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.m26-diag-raw-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.m26-diag-raw-header .label {
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-muted);
}
.m26-diag-raw-code {
  padding: 10px 14px;
  background: #0f172a;
  color: #f1f5f9;
  border-radius: 8px;
  max-height: 160px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.4;
}
.lat-view-err {
  font-size: 10px;
  color: var(--m26-rose);
  font-weight: 700;
  margin-left: 2px;
}

/* ==========================================================================
   macOS 26 White: Components & Contrast Utilities
   ========================================================================== */
.text-main {
  color: var(--m26-text) !important;
}

/* Gateway self test card */
.m26-gateway-self-test-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 14px 18px;
  background: #ffffff;
  border: 1px solid var(--m26-border);
  border-radius: 12px;
  margin-bottom: 20px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
}

.m26-gateway-status-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-gateway-status-badge {
  font-size: 11.5px;
  color: var(--m26-emerald);
  background: rgba(5, 150, 105, 0.08);
  padding: 2px 8px;
  border-radius: 6px;
  font-weight: 500;
}

.m26-gateway-status-badge.offline {
  color: var(--m26-rose);
  background: rgba(225, 29, 72, 0.08);
}

.m26-gateway-creds-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-top: 10px;
  border-top: 1px solid rgba(0, 0, 0, 0.05);
}

.m26-gateway-cred-item {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  background: rgba(248, 250, 252, 0.9);
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 8px;
  transition: all 0.15s ease;
}

.m26-gateway-cred-item:hover {
  background: #ffffff;
  border-color: rgba(0, 0, 0, 0.12);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.03);
}

.m26-gateway-cred-label {
  font-size: 10.5px;
  font-weight: 700;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.05);
  padding: 2px 6px;
  border-radius: 4px;
  letter-spacing: 0.5px;
  flex-shrink: 0;
  user-select: none;
}

.m26-gateway-cred-value {
  flex: 1;
  min-width: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
  color: var(--m26-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
  user-select: text;
  -webkit-user-select: text;
  transition: color 0.15s ease;
}

.m26-gateway-cred-value:hover {
  color: var(--m26-accent);
}

.m26-cred-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.m26-cred-copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 500;
  color: var(--m26-text-secondary);
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.09);
  border-radius: 5px;
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;
}

.m26-cred-copy-btn:hover {
  background: #f1f5f9;
  color: var(--m26-text);
  border-color: rgba(0, 0, 0, 0.18);
}

.m26-cred-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  color: var(--m26-text-muted);
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.09);
  border-radius: 5px;
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;
}

.m26-cred-icon-btn:hover {
  background: #f1f5f9;
  color: var(--m26-text);
  border-color: rgba(0, 0, 0, 0.18);
}

.m26-gateway-self-test-info {
  display: flex;
  align-items: center;
  gap: 12px;
}
.m26-test-pulse {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--m26-emerald);
  box-shadow: 0 0 8px rgba(5, 150, 105, 0.5);
  display: inline-block;
  flex-shrink: 0;
}
.m26-test-pulse.offline {
  background: var(--m26-rose);
  box-shadow: 0 0 8px rgba(225, 29, 72, 0.5);
}

/* Search bar clear button */
.m26-search-clear {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: none;
  background: rgba(0, 0, 0, 0.08);
  color: var(--m26-text-secondary);
  font-size: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-search-clear:hover {
  background: rgba(0, 0, 0, 0.15);
  color: var(--m26-text);
}

/* Input password wrapper & toggle */
.m26-input-wrapper {
  position: relative;
  display: flex;
  align-items: center;
  width: 100%;
}
.m26-input-wrapper .m26-input {
  width: 100%;
  padding-right: 36px;
}
.m26-input-eye {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  background: none;
  border: none;
  color: var(--m26-text-muted);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 4px;
  border-radius: 4px;
  transition: color 0.15s ease;
}
.m26-input-eye:hover {
  color: var(--m26-text);
}

/* Custom interval wrapper */
.m26-custom-interval-wrap {
  width: 100%;
}


/* Provider pills & status indicators */
.prov-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--m26-text-muted);
  display: inline-block;
  flex-shrink: 0;
}
.prov-dot.healthy { background: var(--m26-emerald); }
.prov-dot.degraded { background: var(--m26-amber); }
.prov-dot.offline { background: var(--m26-rose); }
.m26-prov-status-indicator {
  font-size: 11px;
  font-weight: 600;
}
.m26-prov-status-indicator.healthy { color: var(--m26-emerald); }
.m26-prov-status-indicator.degraded { color: var(--m26-amber); }
.m26-prov-status-indicator.offline { color: var(--m26-rose); }

/* Log detail sheet */
.m26-log-detail-sheet {
  max-width: 720px;
  width: 95%;
}
.m26-diag-icon-badge.success {
  background: rgba(5, 150, 105, 0.1);
  color: var(--m26-emerald);
}
.m26-diag-icon-badge.amber {
  background: rgba(217, 119, 6, 0.1);
  color: var(--m26-amber);
}
.m26-diag-icon-badge.info {
  background: rgba(59, 130, 246, 0.1);
  color: #2563eb;
}
.m26-diag-icon-badge.danger {
  background: rgba(225, 29, 72, 0.1);
  color: var(--m26-rose);
}
.m26-diag-dot.success {
  background: var(--m26-emerald);
  box-shadow: 0 0 6px rgba(5, 150, 105, 0.5);
}
.m26-diag-dot.amber {
  background: var(--m26-amber);
  box-shadow: 0 0 6px rgba(217, 119, 6, 0.5);
}
.m26-diag-card.success {
  background: rgba(16, 185, 129, 0.05);
  border: 1px solid rgba(16, 185, 129, 0.25);
}
.m26-diag-card.amber {
  background: rgba(245, 158, 11, 0.06);
  border: 1px solid rgba(245, 158, 11, 0.28);
}
.m26-diag-card.info {
  background: rgba(59, 130, 246, 0.06);
  border: 1px solid rgba(59, 130, 246, 0.25);
}
.m26-diag-card.danger {
  background: rgba(225, 29, 72, 0.05);
  border: 1px solid rgba(225, 29, 72, 0.2);
}
.m26-diag-code-pill.code-success {
  background: rgba(5, 150, 105, 0.12);
  color: var(--m26-emerald);
}
.m26-diag-code-pill.code-amber {
  background: rgba(245, 158, 11, 0.14);
  color: #b45309;
}
.m26-diag-code-pill.code-info {
  background: rgba(59, 130, 246, 0.14);
  color: #2563eb;
}
.m26-diag-code-pill.code-danger {
  background: rgba(225, 29, 72, 0.12);
  color: var(--m26-rose);
}

/* Status pills & failover tags in table */
.m26-status-pill.status-failover {
  background: rgba(245, 158, 11, 0.14);
  color: #b45309;
  border: 1px solid rgba(245, 158, 11, 0.28);
  font-weight: 700;
}
.m26-failover-flow {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  max-width: 100%;
  overflow: hidden;
  white-space: nowrap;
}
.m26-failover-arrow {
  color: #f59e0b;
  font-size: 9px;
  font-weight: 700;
  flex-shrink: 0;
}
.m26-tag-failover-from {
  display: inline-flex;
  align-items: center;
  padding: 1px 5px;
  background: rgba(225, 29, 72, 0.08);
  color: #e11d48;
  border-radius: 4px;
  font-size: 10px;
  font-weight: 500;
  max-width: 130px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-shrink: 0;
}
.m26-tag-failover-to {
  display: inline-flex;
  align-items: center;
  padding: 1px 5px;
  background: rgba(16, 185, 129, 0.1);
  color: #059669;
  border-radius: 4px;
  font-size: 10px;
  font-weight: 600;
  max-width: 100px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-shrink: 0;
}
.m26-channel-name-compact {
  font-size: 11.5px;
  font-weight: 500;
  color: var(--m26-text);
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Modal sections & subtitle bar */
.m26-log-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 4px;
}
.m26-section-subtitle-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 2px;
}
.m26-subtitle-tag {
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  padding: 2px 6px;
  background: #e2e8f0;
  color: #475569;
  border-radius: 4px;
}
.m26-subtitle-title {
  font-size: 12px;
  font-weight: 600;
  color: #334155;
}

/* Execution Trace Timeline */
.m26-trace-timeline {
  display: flex;
  flex-direction: column;
  gap: 0;
  padding: 4px 0;
}
.m26-trace-item {
  display: flex;
  gap: 12px;
  position: relative;
}
.m26-trace-node {
  display: flex;
  flex-direction: column;
  align-items: center;
  width: 24px;
  flex-shrink: 0;
}
.m26-trace-index {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  font-weight: 700;
  background: #e2e8f0;
  color: #475569;
  border: 2px solid #fff;
  box-shadow: 0 1px 3px rgba(0,0,0,0.1);
  z-index: 1;
}
.trace-item-success .m26-trace-index {
  background: #10b981;
  color: #ffffff;
}
.trace-item-fail .m26-trace-index {
  background: #f43f5e;
  color: #ffffff;
}
.m26-trace-line {
  flex: 1;
  width: 2px;
  background: #e2e8f0;
  min-height: 24px;
}
.m26-trace-item:last-child .m26-trace-line {
  display: none;
}
.m26-trace-card {
  flex: 1;
  padding: 10px 12px;
  margin-bottom: 10px;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 10px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.03);
}
.trace-item-success .m26-trace-card {
  border-left: 3px solid #10b981;
}
.trace-item-fail .m26-trace-card {
  border-left: 3px solid #f43f5e;
}
.m26-trace-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
}
.m26-trace-channel-name {
  font-size: 13px;
  font-weight: 700;
  color: #1e293b;
}
.m26-trace-id-badge {
  font-size: 10px;
  color: #64748b;
  background: #f1f5f9;
  padding: 1px 5px;
  border-radius: 4px;
}
.m26-trace-retry-badge {
  font-size: 10px;
  color: #d97706;
  background: #fef3c7;
  padding: 1px 5px;
  border-radius: 4px;
  font-weight: 600;
}
.m26-trace-code-pill {
  font-size: 11px;
  font-weight: 700;
  padding: 1px 6px;
  border-radius: 4px;
}
.m26-trace-latency {
  font-size: 11px;
  font-family: ui-monospace, monospace;
  color: #64748b;
}
.m26-trace-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.m26-trace-spec-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: #64748b;
}
.m26-trace-spec-row .label {
  color: #94a3b8;
  font-size: 11px;
}
.m26-trace-spec-row .val {
  color: #334155;
  font-size: 11px;
}
.m26-trace-spec-row .val-ttft {
  font-size: 10px;
  color: #0284c7;
  background: #e0f2fe;
  padding: 1px 5px;
  border-radius: 3px;
  font-family: ui-monospace, monospace;
}
.m26-trace-error-box {
  margin-top: 6px;
  padding: 6px 8px;
  background: #fff1f2;
  border: 1px solid #ffe4e6;
  border-radius: 6px;
  display: flex;
  align-items: flex-start;
  gap: 6px;
  color: #e11d48;
  word-break: break-all;
}

/* Performance Grid */
.m26-perf-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
}
.m26-perf-card {
  padding: 8px 10px;
  background: #f8fafc;
  border: 1px solid var(--m26-border);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.m26-perf-card .label {
  font-size: 10px;
  color: var(--m26-text-muted);
}
.m26-perf-card .val {
  font-size: 13px;
  font-weight: 700;
  color: var(--m26-text);
}
.m26-perf-card .unit {
  font-size: 10px;
  font-weight: normal;
  color: #64748b;
}

/* ==========================================================================
   Keyboard Shortcuts Cheat Sheet & Card Selection Styles
   ========================================================================== */
.m26-shortcuts-sheet {
  max-width: 580px;
  width: 90%;
}
.m26-shortcuts-icon {
  width: 36px;
  height: 36px;
  border-radius: 10px;
  background: rgba(79, 70, 229, 0.1);
  color: #4f46e5;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.m26-shortcuts-body {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 16px 20px;
  max-height: 70vh;
  overflow-y: auto;
}
.m26-shortcut-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.m26-shortcut-group-title {
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: #64748b;
  margin-bottom: 2px;
}
.m26-shortcut-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  background: #f8fafc;
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 12px;
  padding: 8px 12px;
}
.m26-shortcut-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 5px 0;
  border-bottom: 1px solid rgba(0, 0, 0, 0.04);
}
.m26-shortcut-row:last-child {
  border-bottom: none;
}
.m26-shortcut-desc {
  font-size: 12.5px;
  color: #334155;
  font-weight: 500;
}
.m26-kbd-keys {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}
.m26-kbd-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  font-size: 11px;
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", ui-monospace, SFMono-Regular, monospace;
  font-weight: 600;
  color: #1e293b;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.12);
  border-bottom-width: 2px;
  border-radius: 5px;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}
.m26-kbd-or {
  font-size: 11px;
  color: #94a3b8;
  margin: 0 2px;
}

/* Card Selection state */
.m26-hg-card.is-selected {
  border-color: #4f46e5;
  box-shadow: 0 0 0 2.5px rgba(79, 70, 229, 0.25), 0 8px 24px -4px rgba(79, 70, 229, 0.12);
}
.m26-table tr.is-selected {
  background-color: rgba(79, 70, 229, 0.06);
}

/* ==========================================================================
   Client Integration (Claude & Codex) Styles
   ========================================================================== */
.m26-clients-integration-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px 20px;
  background: #ffffff;
  border: 1px solid var(--m26-border);
  border-radius: 14px;
  margin-bottom: 20px;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.02);
}

.m26-clients-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-bottom: 12px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.05);
}

.m26-clients-icon-box {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: rgba(79, 70, 229, 0.08);
  color: #4f46e5;
}

.m26-clients-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
}

@media (max-width: 860px) {
  .m26-clients-grid {
    grid-template-columns: 1fr;
  }
}

.m26-client-item {
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  background: rgba(248, 250, 252, 0.85);
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 12px;
  padding: 14px 16px;
  gap: 12px;
  transition: all 0.18s ease;
}

.m26-client-item:hover {
  background: #ffffff;
  border-color: rgba(79, 70, 229, 0.25);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.04);
}

.m26-client-item.is-configured {
  border-color: rgba(79, 70, 229, 0.18);
  background: linear-gradient(to bottom right, rgba(255, 255, 255, 0.95), rgba(245, 247, 255, 0.6));
}

.m26-client-top {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
}

.m26-client-logo {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border-radius: 9px;
}

.m26-client-logo.claude {
  background: #fff3eb;
  color: #d97706;
  border: 1px solid rgba(217, 119, 6, 0.15);
}

.m26-client-logo.codex {
  background: #ecfdf5;
  color: #059669;
  border: 1px solid rgba(5, 150, 105, 0.15);
}

.m26-app-badge {
  font-size: 10px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 5px;
  background: rgba(0, 0, 0, 0.05);
  color: #64748b;
}

.m26-app-badge.installed {
  background: rgba(5, 150, 105, 0.08);
  color: #059669;
}

.m26-app-badge.running {
  background: rgba(79, 70, 229, 0.1);
  color: #4f46e5;
  box-shadow: 0 0 0 1px rgba(79, 70, 229, 0.2);
}

.m26-client-badge-pill {
  font-size: 11px;
  font-weight: 600;
  padding: 3px 8px;
  border-radius: 12px;
  background: rgba(148, 163, 184, 0.12);
  color: #64748b;
}

.m26-client-badge-pill.active {
  background: rgba(79, 70, 229, 0.12);
  color: #4f46e5;
  border: 1px solid rgba(79, 70, 229, 0.25);
}

.m26-client-body {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-client-meta-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11.5px;
}

.m26-client-meta-line .label {
  color: #64748b;
}

.m26-client-meta-line .value {
  font-weight: 600;
  color: #1e293b;
}

.m26-client-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
}

.m26-client-chip {
  font-size: 10.5px;
  font-family: ui-monospace, SFMono-Regular, monospace;
  padding: 2px 7px;
  border-radius: 5px;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.08);
  color: #334155;
  max-width: 140px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-client-chip.more {
  background: rgba(0, 0, 0, 0.04);
  color: #64748b;
  font-weight: 600;
}

.m26-client-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 6px;
  border-top: 1px solid rgba(0, 0, 0, 0.04);
}

.m26-launch-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: 8px;
  background: #4f46e5;
  color: #ffffff;
  font-weight: 500;
  border: none;
  cursor: pointer;
  transition: all 0.15s ease;
  box-shadow: 0 1px 2px rgba(79, 70, 229, 0.2);
}

.m26-launch-btn:hover:not(:disabled) {
  background: #4338ca;
  box-shadow: 0 2px 6px rgba(79, 70, 229, 0.3);
}

.m26-launch-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* Modals for Client Config */
.m26-client-sheet {
  max-width: 640px !important;
  width: 90vw;
}

.m26-client-modal-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
}

.m26-client-modal-icon.claude {
  background: #fff3eb;
  color: #d97706;
}

.m26-client-modal-icon.codex {
  background: #ecfdf5;
  color: #059669;
}

.m26-client-cfg-section {
  background: rgba(248, 250, 252, 0.65);
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 10px;
  padding: 12px 14px;
}

.m26-client-model-picker {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 6px;
  max-height: 180px;
  overflow-y: auto;
  padding: 4px;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 8px;
}

.m26-model-pick-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.12s ease;
  user-select: none;
}

.m26-model-pick-item:hover {
  background: rgba(0, 0, 0, 0.03);
}

.m26-model-pick-item.selected {
  background: rgba(79, 70, 229, 0.08);
}

.m26-checkbox-custom {
  display: inline-block;
  width: 14px;
  height: 14px;
  border: 1.5px solid #cbd5e1;
  border-radius: 3px;
  background: #ffffff;
  flex-shrink: 0;
  transition: all 0.12s ease;
}

.m26-checkbox-custom.checked {
  background: #4f46e5;
  border-color: #4f46e5;
  box-shadow: inset 0 0 0 2px #ffffff;
}

.m26-model-chan-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 1.5px 6px;
  border-radius: 4px;
  font-size: 10px;
  background: rgba(0, 0, 0, 0.04);
  color: #64748b;
  flex-shrink: 0;
  line-height: 1.2;
}

.m26-model-chan-badge.is-healthy {
  background: rgba(16, 185, 129, 0.1);
  color: #059669;
}

.m26-model-chan-badge.is-degraded {
  background: rgba(245, 158, 11, 0.1);
  color: #d97706;
}

.m26-model-chan-badge.is-failed {
  background: rgba(239, 68, 68, 0.1);
  color: #dc2626;
}

.m26-status-dot-sm {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #94a3b8;
  display: inline-block;
}

.m26-status-dot-sm.is-healthy {
  background: #10b981;
}

.m26-status-dot-sm.is-degraded {
  background: #f59e0b;
}

.m26-status-dot-sm.is-failed {
  background: #ef4444;
}

.m26-chan-name {
  font-weight: 500;
  max-width: 68px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-chan-latency {
  font-size: 9.5px;
  opacity: 0.85;
}

.m26-text-btn {
  background: none;
  border: none;
  font-size: 11.5px;
  color: #4f46e5;
  cursor: pointer;
  padding: 0;
}

.m26-text-btn:hover {
  text-decoration: underline;
}

.m26-slot-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-slot-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 10px;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 8px;
  gap: 10px;
}

.m26-slot-info {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 180px;
}

.m26-slot-tag {
  font-family: ui-monospace, SFMono-Regular, monospace;
  font-size: 10.5px;
  font-weight: 600;
  color: #4f46e5;
  background: rgba(79, 70, 229, 0.08);
  padding: 1px 6px;
  border-radius: 4px;
}

.m26-slot-label {
  font-size: 11.5px;
  color: #475569;
}

.m26-client-notice-box {
  padding: 8px 12px;
  background: #f8fafc;
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 8px;
}

.m26-stream-status-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 9999px;
  background: rgba(16, 185, 129, 0.1);
  border: 1px solid rgba(16, 185, 129, 0.3);
  color: #059669;
}

.m26-stream-status-pill:not(.connected) {
  background: rgba(245, 158, 11, 0.1);
  border-color: rgba(245, 158, 11, 0.3);
  color: #d97706;
}

.m26-btn-active-blue {
  background: rgba(59, 130, 246, 0.12) !important;
  color: #2563eb !important;
  border-color: rgba(59, 130, 246, 0.35) !important;
}

.m26-row-in-flight {
  background: rgba(16, 185, 129, 0.04) !important;
  box-shadow: inset 3px 0 0 #10b981;
}

.status-streaming {
  background: rgba(16, 185, 129, 0.15) !important;
  color: #059669 !important;
  border: 1px solid rgba(16, 185, 129, 0.35) !important;
  animation: pulse-stream 1.8s infinite;
}

@keyframes pulse-stream {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.8; transform: scale(0.98); }
}

/* ==========================================================================
   Analytics Dashboard Styles
   ========================================================================== */
.m26-analytics-header-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 16px;
  padding: 4px 0 6px;
}

.m26-time-pills {
  display: inline-flex;
  align-items: center;
  background: rgba(0, 0, 0, 0.04);
  padding: 3px;
  border-radius: 9px;
  border: 1px solid var(--m26-border);
}

.m26-time-pill-btn {
  padding: 4px 11px;
  font-size: 11.5px;
  font-weight: 500;
  color: var(--m26-text-secondary);
  border-radius: 6px;
  border: none;
  background: transparent;
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-time-pill-btn:hover {
  color: var(--m26-text);
}

.m26-time-pill-btn.active {
  background: #ffffff;
  color: var(--m26-text);
  font-weight: 600;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}

.m26-analytics-kpi-grid {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  gap: 14px;
}

@media (max-width: 1100px) {
  .m26-analytics-kpi-grid {
    grid-template-columns: repeat(3, 1fr);
  }
}

@media (max-width: 768px) {
  .m26-analytics-kpi-grid {
    grid-template-columns: 1fr;
  }
}

.m26-kpi-card {
  padding: 16px 18px;
  gap: 10px;
}

.m26-kpi-label {
  font-size: 12px;
  font-weight: 500;
  color: var(--m26-text-secondary);
}

.m26-kpi-icon-badge {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.m26-kpi-icon-badge.blue { background: rgba(59, 130, 246, 0.1); color: #2563eb; }
.m26-kpi-icon-badge.purple { background: rgba(147, 51, 234, 0.1); color: #9333ea; }
.m26-kpi-icon-badge.amber { background: rgba(217, 119, 6, 0.1); color: #d97706; }
.m26-kpi-icon-badge.cyan { background: rgba(8, 145, 178, 0.1); color: #0891b2; }
.m26-kpi-icon-badge.emerald { background: rgba(5, 150, 105, 0.1); color: #059669; }

.m26-kpi-value-row {
  display: flex;
  align-items: baseline;
  gap: 4px;
}

.m26-kpi-big-num {
  font-size: 24px;
  font-weight: 700;
  color: var(--m26-text);
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.02em;
}

.m26-kpi-unit {
  font-size: 11px;
  color: var(--m26-text-muted);
}

.m26-kpi-footer {
  display: flex;
  align-items: center;
  gap: 6px;
}

.m26-kpi-badge {
  font-size: 11px;
  font-weight: 500;
  padding: 1px 6px;
  border-radius: 4px;
}
.m26-kpi-badge.emerald { background: rgba(5, 150, 105, 0.08); color: var(--m26-emerald); }
.m26-kpi-badge.rose { background: rgba(225, 29, 72, 0.08); color: var(--m26-rose); }
.m26-kpi-badge.amber { background: rgba(217, 119, 6, 0.08); color: var(--m26-amber); }

/* Charts Row */
.m26-analytics-charts-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
}

@media (max-width: 900px) {
  .m26-analytics-charts-grid {
    grid-template-columns: 1fr;
  }
}

.m26-analytics-card {
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  border: 1px solid var(--m26-border);
  border-radius: 14px;
  padding: 18px 20px;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.02);
}

.m26-card-header-sub {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;
  flex-wrap: wrap;
  gap: 10px;
}

.m26-card-sub-icon {
  width: 7px;
  height: 7px;
  border-radius: 50%;
}
.m26-card-sub-icon.blue { background: #3b82f6; }
.m26-card-sub-icon.purple { background: #9333ea; }

.m26-legend-dot {
  width: 7px;
  height: 7px;
  border-radius: 2px;
  display: inline-block;
}
.m26-legend-dot.emerald { background: #10b981; }
.m26-legend-dot.rose { background: #f43f5e; }
.m26-legend-dot.purple { background: #8b5cf6; }
.m26-legend-dot.cyan { background: #06b6d4; }

.m26-chart-container {
  height: 160px;
  display: flex;
  align-items: flex-end;
  padding: 10px 4px 4px;
}

.m26-chart-bars-wrap {
  display: flex;
  align-items: flex-end;
  width: 100%;
  height: 100%;
  gap: 12px;
  justify-content: space-around;
}

.m26-bar-column-group {
  flex: 1;
  max-width: 44px;
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-end;
  position: relative;
  cursor: pointer;
}

.m26-bar-track {
  width: 100%;
  height: 120px;
  display: flex;
  flex-direction: column-reverse;
  align-items: center;
  background: rgba(0, 0, 0, 0.02);
  border-radius: 6px;
  overflow: hidden;
  padding: 2px 0 0;
  transition: background 0.15s ease;
}

.m26-bar-column-group:hover .m26-bar-track {
  background: rgba(0, 0, 0, 0.05);
}

.m26-bar-fill {
  width: 100%;
  transition: height 0.3s cubic-bezier(0.16, 1, 0.3, 1);
  border-radius: 2px;
}
.m26-bar-fill.success {
  background: linear-gradient(180deg, #34d399 0%, #10b981 100%);
}
.m26-bar-fill.failed {
  background: linear-gradient(180deg, #fb7185 0%, #f43f5e 100%);
}
.m26-bar-fill.purple {
  background: linear-gradient(180deg, #a78bfa 0%, #8b5cf6 100%);
}
.m26-bar-fill.cyan {
  background: linear-gradient(180deg, #38bdf8 0%, #06b6d4 100%);
}

.m26-bar-label {
  font-size: 10.5px;
  color: var(--m26-text-muted);
  margin-top: 8px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.m26-bar-tooltip {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 50%;
  transform: translateX(-50%);
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.1);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.12);
  border-radius: 8px;
  padding: 8px 12px;
  min-width: 160px;
  z-index: 40;
  pointer-events: none;
  opacity: 0;
  visibility: hidden;
  transition: opacity 0.15s ease, transform 0.15s ease;
}

.m26-bar-column-group:hover .m26-bar-tooltip {
  opacity: 1;
  visibility: visible;
  transform: translateX(-50%) translateY(-2px);
}

.m26-analytics-empty {
  height: 120px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--m26-text-muted);
  font-size: 12.5px;
}

/* Models Breakdown Table */
.m26-model-filter-input-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  background: #ffffff;
  border: 1px solid var(--m26-border);
  border-radius: 7px;
  padding: 4px 10px;
  width: 220px;
}

.m26-sub-search-input {
  border: none;
  background: transparent;
  outline: none;
  font-size: 11.5px;
  color: var(--m26-text);
  width: 100%;
}

.m26-analytics-table-wrap {
  overflow-x: auto;
  margin-top: 4px;
}

.m26-analytics-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}

.m26-analytics-table th {
  padding: 9px 12px;
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-muted);
  border-bottom: 1px solid var(--m26-border);
}

.m26-analytics-row td {
  padding: 12px 12px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.04);
  vertical-align: middle;
}

.m26-analytics-row:hover td {
  background: rgba(0, 0, 0, 0.015);
}

.m26-model-icon-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #3b82f6;
  flex-shrink: 0;
}

.m26-channel-tag {
  background: rgba(0, 0, 0, 0.05);
  padding: 1px 5px;
  border-radius: 4px;
  font-size: 10px;
  color: var(--m26-text-secondary);
}

.m26-mini-progress-bar {
  width: 100%;
  height: 4px;
  background: rgba(0, 0, 0, 0.05);
  border-radius: 999px;
  overflow: hidden;
}

.m26-mini-progress-fill {
  height: 100%;
  background: linear-gradient(90deg, #3b82f6, #06b6d4);
  border-radius: 999px;
}

.m26-latency-pill {
  font-size: 11.5px;
  font-weight: 600;
  padding: 2px 7px;
  border-radius: 5px;
}

.m26-tag-fast {
  background: rgba(5, 150, 105, 0.08);
  color: #059669;
}

.m26-tag-normal {
  background: rgba(217, 119, 6, 0.08);
  color: #d97706;
}

.m26-tag-slow {
  background: rgba(225, 29, 72, 0.08);
  color: #e11d48;
}

/* Channel Performance Grid */
.m26-channel-perf-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 12px;
}

.m26-channel-perf-card {
  background: #ffffff;
  border: 1px solid var(--m26-border);
  border-radius: 10px;
  padding: 12px 14px;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.m26-channel-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #10b981;
}
</style>
