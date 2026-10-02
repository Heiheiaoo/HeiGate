<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { clearValue, readValue, writeValue } from '@/services/storage'
import { checkGatewayHealth } from '@/services/health'
import {
  clearLogs,
  createChannel,
  deleteChannel,
  duplicateChannel as apiDuplicateChannel,
  fetchUpstreamModels as apiFetchUpstreamModels,
  getChannel,
  getGatewayKey,
  injectionCheck,
  type ModelInjectionResult,
  getSystemInfo,
  listChannels,
  listLogs,
  probeChannelDetails,
  pruneLogs,
  subscribeLogs,
  updateChannel,
  updateSettings,
  getAnalytics,
  getCustomModelMappings,
  setCustomModelMapping,
  deleteCustomModelMapping,
  type ApiChannel,
  type RequestLog,
  type SystemInfo,
  type AnalyticsResult,
} from '@/services/gatewayApi'
import {
  getCanonicalGroupKey as modelGroupKey,
  isModelEquivalent,
  selectCanonicalDisplayName,
} from '@/services/modelNaming'
import { vFocusTrap, openModalCount } from '@/directives/focusTrap'
import ChannelToolbar from '@/components/channels/ChannelToolbar.vue'
import ChannelGridView from '@/components/channels/ChannelGridView.vue'
import ChannelTableView from '@/components/channels/ChannelTableView.vue'
import ChannelModelManager from '@/components/channels/ChannelModelManager.vue'
import ChannelDiagnosticContent from '@/components/channels/ChannelDiagnosticContent.vue'
import ChannelModelLatencyContent from '@/components/channels/ChannelModelLatencyContent.vue'
import ChannelProviderSpeedTestContent from '@/components/channels/ChannelProviderSpeedTestContent.vue'
import ChannelInjectionContent from '@/components/channels/ChannelInjectionContent.vue'
import ChannelModelAliasContent from '@/components/channels/ChannelModelAliasContent.vue'
import ModelMappingContent from '@/components/channels/ModelMappingContent.vue'
import AnalyticsOverviewMetrics from '@/components/analytics/AnalyticsOverviewMetrics.vue'
import AnalyticsModelBreakdown from '@/components/analytics/AnalyticsModelBreakdown.vue'
import AnalyticsChannelPerformance from '@/components/analytics/AnalyticsChannelPerformance.vue'
import AnalyticsTrendCharts from '@/components/analytics/AnalyticsTrendCharts.vue'
import LogDetailContent from '@/components/logs/LogDetailContent.vue'
import LogsView from '@/components/logs/LogsView.vue'
import RoutingSettings from '@/components/settings/RoutingSettings.vue'
import GatewayAccessSettings from '@/components/settings/GatewayAccessSettings.vue'
import StorageSettings from '@/components/settings/StorageSettings.vue'
import ModelRulesSettings from '@/components/settings/ModelRulesSettings.vue'
import type { Channel, ChannelFilterStatus, ModelLatencyRow, ModelProviderSpeedTestRow } from '@/components/channels/types'
// Re-exported so existing imports of Channel from this view keep working; the
// canonical definition lives in components/channels/types.ts.
export type { Channel }
export type GlobalRouteStrategy = 'smart_quality' | 'latency' | 'round_robin'

// Tauri injects its IPC bridge into every page it loads; in a plain browser
// (the Vite dev server) it is absent and the UI draws its own window chrome.
const isDesktop = ref(typeof window !== 'undefined' && Boolean((window as any).__TAURI_INTERNALS__))
const savedSection = typeof localStorage !== 'undefined' ? (localStorage.getItem('heigate_active_section') as any) : null
const activeSection = ref<'analytics' | 'channels' | 'models' | 'logs' | 'settings'>(
  savedSection && ['analytics', 'channels', 'models', 'logs', 'settings'].includes(savedSection) ? savedSection : 'analytics'
)
watch(activeSection, async (val) => {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('heigate_active_section', val)
  }
  // Single dispatch point for per-section loads. Keeping this in one watcher
  // avoids firing each section's fetches twice on every tab switch.
  if (val === 'analytics') {
    void loadAnalytics()
    void refreshGatewayHealth()
  } else if (val === 'logs') {
    void refreshLogs()
  } else if (val === 'settings') {
    void refreshSystemInfo()
    void refreshGatewayHealth()
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
  ttft?: number | null
  probing?: boolean
  status?: string
  error?: string | null
  message?: string
  checkedAt?: string | null
}>>({})
const isProbingAllModels = ref(false)

// Model Multi-Provider Speed Test Modal
const activeModelSpeedTestName = ref<string | null>(null)
const isProbingModelProviders = ref(false)
const modelSpeedTestQuery = ref('')





// Analytics state & helpers
const analyticsDays = ref(7)
const analyticsData = ref<AnalyticsResult | null>(null)
const isLoadingAnalytics = ref(false)
const analyticsModelSearch = ref('')

async function loadAnalytics() {
  isLoadingAnalytics.value = true
  try {
    const data = await getAnalytics(analyticsDays.value)
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

// 所有延迟展示一律用秒（0.33s / 2.42s / 23.9s），不再出现毫秒。
function formatLatencySec(ms: number | null | undefined): string {
  if (ms == null || isNaN(ms) || ms <= 0) return '—'
  const sec = ms / 1000
  return (sec >= 10 ? sec.toFixed(1) : sec.toFixed(2)) + 's'
}

function formatAnalyticsLatency(ms: number): string {
  if (!ms || isNaN(ms) || ms <= 0) return '-'
  const sec = ms / 1000
  return (sec >= 10 ? sec.toFixed(1) : sec.toFixed(2)) + 's'
}

// 协议 pill 的短标签：完整枚举值（如 OPENAI-COMPATIBLE）在表格列里放不下。
function protocolShortLabel(protocol: Channel['protocol']): string {
  if (protocol === 'anthropic') return 'Anthropic'
  if (protocol === 'openai-responses') return 'Responses'
  return 'OpenAI Chat'
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
const channelViewMode = ref<'grid' | 'list'>('grid')
function setChannelViewMode(mode: 'grid' | 'list') {
  channelViewMode.value = mode
  try {
    localStorage.setItem('heigate_channel_view_mode', mode)
  } catch (err) {
    console.debug('Failed to persist view mode', err)
  }
}

// Add Channel Form State
const newChannelName = ref('')
const newChannelUrl = ref('')
const newChannelKey = ref('')
const newChannelProtocol = ref<Channel['protocol']>('openai-compatible')
const newChannelEnabled = ref(true)
const newChannelPrimaryModel = ref('')
const newChannelModelNames = ref<string[]>([])
const isFetchingNewModels = ref(false)
const showNewKey = ref(false)

// Edit Channel Form State
const editChannelName = ref('')
const editChannelUrl = ref('')
const editChannelKey = ref('')
const editChannelProtocol = ref<Channel['protocol']>('openai-compatible')
const editChannelEnabled = ref(true)
const editChannelPrimaryModel = ref('')
const editChannelModelNames = ref<string[]>([])
const isFetchingEditModels = ref(false)
const showEditKey = ref(false)

const configInput = ref<HTMLInputElement | null>(null)
const gatewayApiActive = ref(false)
const gatewayOnline = ref(false)
const gatewayLatency = ref<number | null>(null)
const appVersion = ref<string>(__HEIGATE_APP_VERSION__)
const gatewayLastChecked = ref('尚未检测')
const gatewayApiKey = ref('')
const maskedOverviewApiKey = computed(() => {
  const k = gatewayApiKey.value
  if (!k) return '••••••••'
  if (k.length <= 16) return '••••••••'
  return `${k.slice(0, 8)}••••••••${k.slice(-6)}`
})

// System Info & Storage & Logs
const systemInfo = ref<SystemInfo | null>(null)
const logRetentionDays = ref<number>(7)
const logMaxCount = ref<number>(5000)
const isPruningLogs = ref(false)
const isClearingLogs = ref(false)
const desktopLogs = ref<RequestLog[]>([])
const activeLogDetail = ref<RequestLog | null>(null)
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
  if (import.meta.env.DEV) return __HEIGATE_DEV_GATEWAY_URL__
  if (typeof window === 'undefined') return 'http://127.0.0.1:18080/v1'
  return `${window.location.origin}/v1`
})


const channels = ref<Channel[]>([])

const channelStatusFilter = ref<ChannelFilterStatus>('all')
const channelToolbarRef = ref<InstanceType<typeof ChannelToolbar> | null>(null)

const healthyChannelsCount = computed(() => channels.value.filter(c => c.enabled && c.status === 'healthy').length)
const degradedChannelsCount = computed(() => channels.value.filter(c => c.enabled && c.status === 'degraded').length)
const errorChannelsCount = computed(() => channels.value.filter(c => c.enabled && c.status !== 'healthy' && c.status !== 'degraded').length)
const disabledChannelsCount = computed(() => channels.value.filter(c => !c.enabled).length)

const filteredChannels = computed(() => {
  const query = channelSearch.value.trim().toLowerCase()
  return channels.value.filter((channel) => {
    const matchesQuery = !query || `${channel.name} ${channel.url} ${(channel.modelNames || []).join(' ')}`.toLowerCase().includes(query)
    const matchesProtocol = protocolFilter.value === 'all' || channel.protocol === protocolFilter.value
    let matchesStatus = true
    if (channelStatusFilter.value === 'healthy') {
      matchesStatus = channel.enabled && channel.status === 'healthy'
    } else if (channelStatusFilter.value === 'degraded') {
      matchesStatus = channel.enabled && channel.status === 'degraded'
    } else if (channelStatusFilter.value === 'offline') {
      matchesStatus = channel.enabled && channel.status !== 'healthy' && channel.status !== 'degraded'
    } else if (channelStatusFilter.value === 'disabled') {
      matchesStatus = !channel.enabled
    }
    return matchesQuery && matchesProtocol && matchesStatus
  })
})




const sortedChannels = computed(() => {
  const list = [...filteredChannels.value]
  return list.sort((a, b) => {
    if (a.enabled !== b.enabled) return a.enabled ? -1 : 1
    const statusWeight = (s: Channel['status']) => (s === 'healthy' ? 0 : s === 'degraded' ? 1 : 2)
    const swA = statusWeight(a.status)
    const swB = statusWeight(b.status)
    if (swA !== swB) return swA - swB
    const resp = (c: Channel) => (c.ttft != null && c.ttft > 0 ? c.ttft : (c.latency != null && c.latency > 0 ? c.latency : 999999))
    const latA = resp(a)
    const latB = resp(b)
    if (latA !== latB) return latA - latB
    return (a.name || '').localeCompare(b.name || '')
  })
})

const enabledChannels = computed(() => channels.value.filter((channel) => channel.enabled))

const customModelMappings = ref<Record<string, string>>({})

async function loadCustomModelMappings() {
  try {
    const data = await getCustomModelMappings()
    customModelMappings.value = data || {}
  } catch {
    customModelMappings.value = {}
  }
}

// Thin wrappers binding the pure helpers in services/modelNaming.ts to the
// current custom-mapping dictionary. The normalization logic and its tests live
// in that module; these just supply the reactive mappings.
function getCanonicalGroupKey(raw: string): string {
  return modelGroupKey(raw, customModelMappings.value)
}

function isModelEquivalentFrontend(a: string, b: string): boolean {
  return isModelEquivalent(a, b, customModelMappings.value)
}

function selectCanonicalDisplayNameFrontend(models: string[]): string {
  return selectCanonicalDisplayName(models, customModelMappings.value)
}

interface AggregateModelCard {
  name: string
  canonicalKey: string
  rawNames: string[]
  aliases: string[]
  providerCount: number
  status: 'healthy' | 'degraded' | 'unknown' | 'offline'
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
    status: 'healthy' | 'degraded' | 'unknown' | 'offline'
  }>()

  channels.value.forEach((channel) => {
    // Disabled suppliers are completely absent from the model catalog: they
    // must not advertise models, count as providers, or participate in probes.
    if (!channel.enabled) return
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
      const isPrimary = isPrimaryModel(channel, rawModelName)
      const effectiveLat = specificEntry?.latency != null
        ? (specificEntry.ttft ?? specificEntry.latency)
        : (isPrimary ? (channel.ttft ?? channel.latency) : null)
      const effectiveStatus = specificEntry?.status
        ? (specificEntry.status === 'operational' ? 'healthy' : specificEntry.status === 'degraded' ? 'degraded' : 'offline')
        : (isPrimary ? channel.status : 'unknown')

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
      else if (effectiveStatus === 'unknown' && current.status === 'offline') current.status = 'unknown'
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
      latency: formatLatencySec(avgLat),
    })
  })

  const autoChannels = enabledChannels.value
  if (autoChannels.length > 0) {
    const details = autoChannels.map((channel) => ({
      name: channel.name,
      status: channel.status,
      latency: channel.ttft ?? channel.latency,
      protocol: channel.protocol,
      upstreamModel: channel.primaryModel || channel.modelNames?.[0] || '',
    }))
    const latencies = details.map((item) => item.latency).filter((value): value is number => value != null && value > 0)
    const status = details.some((item) => item.status === 'healthy')
      ? 'healthy'
      : details.some((item) => item.status === 'degraded') ? 'degraded'
        : details.some((item) => item.status === 'unknown') ? 'unknown' : 'offline'
    const avgLatency = latencies.length ? Math.round(latencies.reduce((sum, value) => sum + value, 0) / latencies.length) : null
    list.unshift({
      name: 'Auto',
      canonicalKey: 'auto',
      rawNames: ['Auto'],
      aliases: [],
      providerCount: details.length,
      status,
      latencies,
      providers: details.map((item) => item.name),
      channelDetails: details,
      avgLatency,
      latency: formatLatencySec(avgLatency),
    })
  }

  return list.sort((a, b) => {
    const statusWeight = (s: AggregateModelCard['status']) => (s === 'healthy' ? 0 : s === 'degraded' ? 1 : s === 'unknown' ? 2 : 3)
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

function openClassifyDialog(sourceModelName: string, defaultTarget = '') {
  classifySourceModel.value = sourceModelName
  classifyTargetModel.value = defaultTarget || (existingCanonicalModelNames.value[0] || '')
  showClassifyModal.value = true
}

async function submitClassify({ source, target }: { source: string; target: string }) {
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

function openCustomMappingsModal() {
  showCustomMappingsModal.value = true
}

async function handleAddManagerMapping({ source, target }: { source: string; target: string }) {
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
      channels: matchedChannels.length > 0
        ? matchedChannels.map((ch) => ({
          name: ch.name,
          status: ch.status,
          latency: ch.ttft ?? ch.latency,
          protocol: ch.protocol,
          upstreamModel: alias,
        }))
        : m.channelDetails.filter((c) => c.upstreamModel === alias),
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


function toggleChannel(channel: Channel) {
  channel.enabled = !channel.enabled
  void persistChannel(channel)
  showToast(`已${channel.enabled ? '启用' : '暂停'}渠道“${channel.name}”`)
}

function fromDesktopChannel(item: ApiChannel): Channel {
  const modelNames = [item.primaryModel, ...item.extraModels].filter(Boolean)
  return {
    name: item.name,
    url: item.url,
    protocol: item.provider,
    status: item.status,
    latency: item.latency,
    ttft: item.ttft ?? null,
    models: modelNames.length,
    lastCheck: item.lastCheck,
    accent: item.provider === 'anthropic' ? '#f59e0b' : item.provider === 'openai-responses' ? '#3b82f6' : '#10b981',
    enabled: item.enabled,
    apiKey: item.apiKey,
    primaryModel: item.primaryModel,
    modelNames,
    probing: false,
    gatewayId: item.id,
    failureCount: item.failureCount,
    circuitState: item.circuitState,
    cooldownUntil: item.cooldownUntil,
    lastError: item.lastError,
    injectionState: item.injectionState ?? 'unknown',
    injectionTokens: item.injectionTokens ?? null,
    injectionContent: item.injectionContent ?? '',
    injectionCheckedAt: item.injectionCheckedAt ?? null,
    modelHealth: item.modelHealth ?? [],
  }
}

function toDesktopChannel(channel: Channel): ApiChannel {
  const primary = channel.primaryModel || channel.modelNames?.[0] || 'gpt-4o-mini'
  const extras = (channel.modelNames || []).filter((m) => m !== primary)
  return {
    id: channel.gatewayId ?? 0,
    name: channel.name,
    provider: channel.protocol,
    apiMode: channel.protocol === 'openai-responses' ? 'responses' : 'chat_completions',
    url: channel.url,
    apiKey: channel.apiKey,
    primaryModel: primary,
    extraModels: extras,
    enabled: channel.enabled,
    status: channel.status,
    latency: channel.latency,
    ttft: channel.ttft ?? null,
    pingLatency: null,
    lastError: '',
    lastCheck: channel.lastCheck,
    failureCount: channel.failureCount ?? 0,
    circuitState: channel.circuitState ?? 'closed',
    cooldownUntil: channel.cooldownUntil ?? null,
    injectionState: channel.injectionState ?? 'unknown',
    injectionTokens: channel.injectionTokens ?? null,
    injectionContent: channel.injectionContent ?? '',
    injectionCheckedAt: channel.injectionCheckedAt ?? null,
    modelHealth: channel.modelHealth ?? [],
  }
}

async function refreshChannels() {
  const remote = await listChannels()
  if (remote === null) return false
  gatewayApiActive.value = true
  channels.value = remote.map(fromDesktopChannel)
  for (const channel of channels.value) {
    for (const health of channel.modelHealth ?? []) {
      const key = getModelKey(channel, health.model)
      if (!modelLatencies.value[key]?.probing) {
        modelLatencies.value[key] = {
          latency: health.latencyMs,
          ttft: health.ttftMs,
          probing: false,
          status: health.status,
          error: health.error || null,
          message: health.error || (health.status === 'degraded' ? '响应稍慢' : ''),
          checkedAt: health.checkedAt,
        }
      }
    }
  }
  return true
}

async function persistChannel(channel: Channel) {
  if (!gatewayApiActive.value) {
    await saveChannels()
    return
  }
  const saved = await updateChannel(toDesktopChannel(channel))
  // 单次失败只提示，不切换到本地存储模式：一旦切换，后续编辑会写进本地，
  // 下次 refreshChannels 拉取服务端数据时这些编辑会被静默丢弃。
  if (!saved) showToast(`渠道“${channel.name}”保存失败，请检查网关状态后重试`)
}

async function probeChannel(channel: Channel) {
  if (channel.probing) return
  channel.probing = true
  if (gatewayApiActive.value && channel.gatewayId) {
    const details = await probeChannelDetails(channel.gatewayId)
    if (details === 'busy') {
      channel.probing = false
      showToast(`渠道“${channel.name}”已有探活正在进行，请稍候刷新查看`)
      return
    }
    if (details === 'disabled') {
      channel.probing = false
      showToast(`渠道“${channel.name}”已停用，无法探活`)
      return
    }
    if (details === null) {
      channel.probing = false
      showToast(`渠道“${channel.name}”探活请求失败，请检查网关状态`)
      return
    }
    if (details.length === 0) {
      // 同渠道已有探活在飞行中: 后端返回空结果而不是排队等待。
      channel.probing = false
      showToast(`渠道“${channel.name}”已有探活正在进行，请稍候刷新查看`)
      return
    }
    if (details.length > 0) {
      for (const res of details) {
        const key = getModelKey(channel, res.model)
        const isOk = res.status === 'operational' || res.status === 'degraded' || res.status === 'healthy'
        const hasErr = !isOk
        const errMsg = hasErr ? (res.message || '模型调用异常') : null
        modelLatencies.value[key] = {
          latency: res.latencyMs ?? null,
          ttft: res.ttftMs ?? null,
          probing: false,
          status: res.status,
          error: errMsg,
          message: hasErr ? errMsg! : (res.status === 'degraded' ? '响应稍慢' : ''),
          checkedAt: new Date().toISOString(),
        }
      }
      const primaryRes = details.find((d) => isPrimaryModel(channel, d.model)) || details[0]
      if (primaryRes) {
        const isOk = primaryRes.status === 'operational' || primaryRes.status === 'degraded' || primaryRes.status === 'healthy'
        channel.latency = primaryRes.latencyMs ?? null
        channel.ttft = primaryRes.ttftMs ?? null
        if (isOk) {
          channel.status = primaryRes.status === 'degraded' ? 'degraded' : 'healthy'
          channel.lastError = ''
        } else {
          channel.status = 'offline'
          channel.lastError = primaryRes.message || '主模型探活失败'
        }
      }
    }
    await refreshChannels()
    channel.probing = false
    const primaryRes = details?.find((d) => isPrimaryModel(channel, d.model)) || details?.[0]
    const pModelName = primaryRes?.model || channel.primaryModel || '主模型'
    if (primaryRes && (primaryRes.status === 'healthy' || primaryRes.status === 'operational')) {
      showToast(`渠道“${channel.name}”探活成功，主模型 [${pModelName}] 状态就绪`)
    } else if (primaryRes && primaryRes.status === 'degraded') {
      showToast(`渠道“${channel.name}”探活完成，主模型 [${pModelName}] 响应稍慢`)
    } else {
      showToast(`渠道“${channel.name}”探活异常，主模型 [${pModelName}] 调用失败`)
    }
    return
  }
  channel.probing = false
  showToast(`渠道“${channel.name}”无法探活：本地网关未连接`)
}

const isProbingAllChannels = ref(false)

async function probeAllChannels() {
  if (isProbingAllChannels.value) return
  isProbingAllChannels.value = true
  try {
    for (const channel of enabledChannels.value) {
      await probeChannel(channel)
    }
    showToast('全部渠道主模型探活完成')
  } finally {
    isProbingAllChannels.value = false
  }
}

async function duplicateChannel(channel: Channel) {
  if (gatewayApiActive.value && channel.gatewayId) {
    const duplicated = await apiDuplicateChannel(channel.gatewayId)
    if (duplicated) {
      await refreshChannels()
      showToast(`已成功复制渠道“${channel.name}”`)
      return
    }
    // 服务端复制失败：不要落到本地 push——那份副本不在服务端，下次刷新即消失。
    showToast(`复制渠道“${channel.name}”失败，请检查网关状态后重试`)
    return
  }
  const copy: Channel = {
    ...channel,
    name: `${channel.name} (副本)`,
    gatewayId: undefined,
    status: 'offline',
    latency: null,
    ttft: null,
    lastCheck: '尚未探活',
  }
  channels.value.push(copy)
  await saveChannels()
  showToast(`已成功复制渠道“${channel.name}”`)
}

// ---- 注入检测 ----
const showInjectionModal = ref(false)
const injectionChannel = ref<Channel | null>(null)
const isCheckingInjection = ref(false)
const injectionResults = ref<ModelInjectionResult[]>([])
const modelInjections = ref<Record<string, ModelInjectionResult>>({})
const modelCheckingInjection = ref<Record<string, boolean>>({})
const isCheckingAllInjections = ref(false)
const copiedInjectionText = ref('')

function copyInjectionContent(content?: string) {
  if (!content) return
  navigator.clipboard.writeText(content).then(() => {
    copiedInjectionText.value = content
    showToast('已复制注入 Prompt 原文到剪贴板')
    setTimeout(() => {
      if (copiedInjectionText.value === content) copiedInjectionText.value = ''
    }, 2000)
  })
}

function getModelInjection(channel: Channel, modelName: string): ModelInjectionResult | null {
  const key = getModelKey(channel, modelName)
  const r = modelInjections.value[key]
  if (r) return r
  if (isPrimaryModel(channel, modelName) && channel.injectionState && channel.injectionState !== 'unknown') {
    return {
      model: modelName,
      state: channel.injectionState,
      prompt_tokens: channel.injectionTokens ?? 0,
      content: channel.injectionContent,
      checked_at: channel.injectionCheckedAt ?? '',
    }
  }
  return null
}

function isModelCheckingInjection(channel: Channel, modelName: string): boolean {
  return Boolean(modelCheckingInjection.value[getModelKey(channel, modelName)])
}

function getModelInjectionClass(channel: Channel, modelName: string): string {
  if (isModelCheckingInjection(channel, modelName)) return 'checking'
  const inj = getModelInjection(channel, modelName)
  if (!inj) return 'idle'
  return inj.state || 'idle'
}

function getModelInjectionLabel(channel: Channel, modelName: string): string {
  if (isModelCheckingInjection(channel, modelName)) return '检测中...'
  const inj = getModelInjection(channel, modelName)
  if (!inj) return '未检测'
  switch (inj.state) {
    case 'clean': return '输入正常'
    case 'suspected': return `输入偏高 · ${inj.prompt_tokens} tok`
    case 'detected': return `显著偏高 · ${inj.prompt_tokens} tok`
    case 'error': return '检测异常'
    default: return '未检测'
  }
}

function getModelInjectionTooltip(channel: Channel, modelName: string): string {
  const inj = getModelInjection(channel, modelName)
  if (!inj) return '尚未检测。检测会向供应商发送真实请求'
  const checkedAt = inj.checked_at ? ` 检测时间：${formatModelCheckedAt(inj.checked_at)}。` : ''
  if (inj.state === 'clean') return `最小探针请求的输入统计为 ${inj.prompt_tokens} tokens，处于正常模板参考范围。${checkedAt}点击查看详情`
  if (inj.state === 'suspected') return `最小探针请求的输入统计为 ${inj.prompt_tokens} tokens，超过正常模板参考范围；这不能单独证明存在注入。${checkedAt}点击查看详情`
  if (inj.state === 'detected') return `最小探针请求的输入统计为 ${inj.prompt_tokens} tokens，超过检测阈值；内容提取结果仅作辅助证据。${checkedAt}点击查看详情`
  if (inj.state === 'error') return `检测失败：${inj.message || '上游接口异常'}。点击查看详情`
  return '点击检测此模型的 Prompt 注入'
}

async function checkSingleModelInjection(channel: Channel, modelName: string) {
  if (!channel || !channel.gatewayId) return
  const key = getModelKey(channel, modelName)
  if (modelCheckingInjection.value[key]) return
  modelCheckingInjection.value[key] = true
  try {
    const results = await injectionCheck(channel.gatewayId, { model: modelName })
    if (!results || results.length === 0) {
      showToast(`模型 [${modelName}] 注入检测失败`)
      return
    }
    const r = results[0]
    modelInjections.value[key] = r
    if (isPrimaryModel(channel, modelName)) {
      channel.injectionState = r.state
      channel.injectionTokens = r.prompt_tokens
      channel.injectionContent = r.content || r.message || ''
      channel.injectionCheckedAt = r.checked_at
    }
    if (r.state === 'detected') {
      showToast(`模型 [${modelName}] 输入统计显著偏高 (${r.prompt_tokens} tokens)，请查看检测详情`)
    } else if (r.state === 'suspected') {
      showToast(`模型 [${modelName}] 输入统计偏高 (${r.prompt_tokens} tokens)，这不能单独证明注入`)
    } else if (r.state === 'clean') {
      showToast(`模型 [${modelName}] 输入统计在正常参考范围 (${r.prompt_tokens} tokens)`)
    } else {
      showToast(`模型 [${modelName}] 注入检测失败：${r.message || '上游返回异常'}`)
    }
  } catch (err: any) {
    showToast(`注入检测异常: ${err?.message || '未知错误'}`)
  } finally {
    modelCheckingInjection.value[key] = false
  }
}

function requestBatchModelAction(channel: Channel, action: 'probe' | 'injection') {
  const allModels = channel.modelNames ?? []
  const selected = selectedModelNames.value.filter((model) => allModels.includes(model))
  const models = selected.length > 0 ? selected : allModels
  if (models.length === 0) return

  const isProbe = action === 'probe'
  confirmModal.value = {
    title: isProbe ? '批量探活确认' : '批量注入检测确认',
    message: `将对渠道“${channel.name}”的 ${models.length} 个模型发送真实${isProbe ? '探活' : '检测'}请求。`,
    description: isProbe
      ? '请求会逐个执行，每个模型 1 次。可能消耗供应商额度；供应商自身的频率限制仍可能生效。'
      : '请求会逐个执行，每个模型至少 1 次；输入统计偏高时还会追加内容提取请求。可能消耗供应商额度，供应商自身的频率限制仍可能生效。',
    confirmText: `继续 (${models.length} 个模型)`,
    onConfirm: async () => {
      if (isProbe) await probeAllChannelModels(channel, models)
      else await checkModelsInjection(channel, models)
    },
  }
}

async function checkModelsInjection(channel: Channel, models: string[]): Promise<ModelInjectionResult[]> {
  if (!channel.gatewayId || isCheckingAllInjections.value || models.length === 0) return []
  isCheckingAllInjections.value = true
  const results: ModelInjectionResult[] = []
  try {
    let failed = 0
    for (const model of models) {
      const key = getModelKey(channel, model)
      modelCheckingInjection.value[key] = true
      try {
        const modelResults = await injectionCheck(channel.gatewayId, { model })
        if (!modelResults?.length) {
          failed++
          continue
        }
        for (const result of modelResults) {
          modelInjections.value[getModelKey(channel, result.model)] = result
          results.push(result)
          if (isPrimaryModel(channel, result.model)) {
            channel.injectionState = result.state
            channel.injectionTokens = result.prompt_tokens
            channel.injectionContent = result.content || result.message || ''
            channel.injectionCheckedAt = result.checked_at
          }
        }
      } finally {
        modelCheckingInjection.value[key] = false
      }
    }
    const injected = results.filter((result) => result.state === 'detected').length
    const elevated = results.filter((result) => result.state === 'suspected').length
    const errorCount = failed + results.filter((result) => result.state === 'error').length
    showToast(`检测完成 ${results.length}/${models.length}：显著偏高 ${injected}，输入偏高 ${elevated}${errorCount ? `，失败 ${errorCount}` : ''}`)
  } catch (err: any) {
    showToast(`注入检测异常：${err?.message || '未知错误'}`)
  } finally {
    isCheckingAllInjections.value = false
    await refreshChannels()
  }
  return results
}

function openModelInjectionDetail(channel: Channel, modelName: string) {
  const key = getModelKey(channel, modelName)
  let r = modelInjections.value[key]
  if (!r && isPrimaryModel(channel, modelName) && channel.injectionState && channel.injectionState !== 'unknown') {
    r = {
      model: modelName,
      state: channel.injectionState,
      prompt_tokens: channel.injectionTokens ?? 0,
      content: channel.injectionContent,
      checked_at: channel.injectionCheckedAt ?? '',
    }
  }
  if (!r) {
    checkSingleModelInjection(channel, modelName)
    return
  }
  injectionChannel.value = channel
  injectionResults.value = [r]
  showInjectionModal.value = true
}

function openInjectionModal(channel: Channel) {
  injectionChannel.value = channel
  // Seed the list from cached per-model results or the persisted primary-model result
  const list: ModelInjectionResult[] = []
  for (const m of (channel.modelNames || [])) {
    const k = getModelKey(channel, m)
    if (modelInjections.value[k]) {
      list.push(modelInjections.value[k])
    }
  }
  if (list.length > 0) {
    injectionResults.value = list
  } else if (channel.injectionState && channel.injectionState !== 'unknown') {
    injectionResults.value = [{
      model: channel.primaryModel || channel.modelNames?.[0] || '(主模型)',
      state: channel.injectionState,
      prompt_tokens: channel.injectionTokens ?? 0,
      content: channel.injectionContent,
      checked_at: channel.injectionCheckedAt ?? '',
    }]
  } else {
    injectionResults.value = []
  }
  showInjectionModal.value = true
}

async function runInjectionCheck(all: boolean) {
  const channel = injectionChannel.value
  if (!channel || !channel.gatewayId || isCheckingInjection.value) return
  if (all) {
    const models = channel.modelNames ?? []
    confirmModal.value = {
      title: '全模型检测确认',
      message: `将对渠道“${channel.name}”的 ${models.length} 个模型逐个发送真实检测请求。`,
      description: '每个模型至少 1 次请求；输入统计偏高时还会追加内容提取请求。操作按顺序执行，可能消耗供应商额度，供应商自身的频率限制仍可能生效。',
      confirmText: `检测 ${models.length} 个模型`,
      onConfirm: async () => {
        isCheckingInjection.value = true
        try {
          injectionResults.value = await checkModelsInjection(channel, models)
        } finally {
          isCheckingInjection.value = false
        }
      },
    }
    return
  }
  isCheckingInjection.value = true
  const results = await injectionCheck(channel.gatewayId, false)
  isCheckingInjection.value = false
  if (!results) {
    showToast('注入检测请求失败，请检查网关状态')
    return
  }
  injectionResults.value = results
  for (const r of results) {
    const k = getModelKey(channel, r.model)
    modelInjections.value[k] = r
  }

  const primary = results.find((r) => r.model === channel.primaryModel) ?? results[0]
  if (primary) {
    channel.injectionState = primary.state
    channel.injectionTokens = primary.prompt_tokens
    channel.injectionContent = primary.content || primary.message || ''
    channel.injectionCheckedAt = primary.checked_at
  }

  const injected = results.filter((r) => r.state === 'detected').length
  const suspected = results.filter((r) => r.state === 'suspected').length
  if (injected > 0) showToast(`${injected} 个模型输入统计显著偏高${suspected ? `，${suspected} 个模型输入偏高` : ''}，请查看详情`)
  else if (suspected > 0) showToast(`${suspected} 个模型输入统计偏高；这不能单独证明注入`)
  else if (results.every((r) => r.state === 'clean')) showToast(`${results.length} 个模型的输入统计均处于正常模板参考范围`)
  else showToast('注入检测完成，部分模型未能检测')
}

async function openEditChannel(channel: Channel) {
  editingChannel.value = channel
  editChannelName.value = channel.name
  editChannelUrl.value = channel.url

  let rawKey = channel.apiKey || ''
  if ((!rawKey || rawKey.includes('***')) && channel.gatewayId) {
    const fresh = await getChannel(channel.gatewayId, true)
    if (fresh?.apiKey && !fresh.apiKey.includes('***')) {
      rawKey = fresh.apiKey
      channel.apiKey = fresh.apiKey
    }
  }
  editChannelKey.value = rawKey
  editChannelProtocol.value = channel.protocol
  editChannelEnabled.value = channel.enabled
  showEditKey.value = false
  const primary = channel.primaryModel || channel.modelNames?.[0] || 'gpt-4o-mini'
  editChannelPrimaryModel.value = primary
  editChannelModelNames.value = channel.modelNames?.length ? [...channel.modelNames] : [primary]
  showEditChannel.value = true
}

async function saveEditChannel() {
  if (!editingChannel.value) return
  const name = editChannelName.value.trim()
  const rawUrl = editChannelUrl.value.trim()
  const primaryModel = editChannelPrimaryModel.value.trim() || editChannelModelNames.value[0] || 'gpt-4o-mini'
  const modelNames = editChannelModelNames.value.includes(primaryModel)
    ? [primaryModel, ...editChannelModelNames.value.filter(m => m !== primaryModel)]
    : [primaryModel, ...editChannelModelNames.value]
  if (!name || !rawUrl || !primaryModel) {
    showToast('请完整填写渠道名称、Base URL 与主模型')
    return
  }
  const finalUrl = normalizeEndpointUrl(rawUrl)
  const target = editingChannel.value
  const primaryChanged = target.primaryModel !== primaryModel
  target.name = name
  target.url = finalUrl
  target.protocol = editChannelProtocol.value
  target.enabled = editChannelEnabled.value
  target.primaryModel = primaryModel
  target.modelNames = modelNames
  target.models = target.modelNames.length
  target.accent = target.protocol === 'anthropic' ? '#f59e0b' : target.protocol === 'openai-responses' ? '#3b82f6' : '#10b981'

  if (editChannelKey.value.trim() && !editChannelKey.value.includes('***')) {
    target.apiKey = editChannelKey.value.trim()
  } else if (!editChannelKey.value.trim()) {
    target.apiKey = ''
  }

  // If primary model changed, sync target status/latency from cached primary model if exists
  if (primaryChanged) {
    const primaryKey = getModelKey(target, primaryModel)
    const cached = modelLatencies.value[primaryKey]
    if (cached && !cached.probing) {
      const isOk = cached.status === 'operational' || cached.status === 'degraded' || cached.status === 'healthy'
      target.latency = cached.latency
      target.ttft = cached.ttft ?? null
      if (isOk) {
        target.status = cached.status === 'degraded' ? 'degraded' : 'healthy'
        target.lastError = ''
      } else if (cached.status === 'error' || cached.status === 'failed' || cached.status === 'timeout') {
        target.status = 'offline'
        target.lastError = cached.error || cached.message || '主模型调用异常'
      }
    }
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
      if (gatewayApiActive.value && channel.gatewayId) {
        const deleted = await deleteChannel(channel.gatewayId)
        if (!deleted) {
          showToast(`删除渠道“${name}”失败，请重试`)
          return
        }
      }
      channels.value = channels.value.filter((item) => item !== channel)
      if (!gatewayApiActive.value) {
        await saveChannels()
      }
      showToast(`已成功删除渠道“${name}”`)
    },
  }
}

function saveChannels() {
  return writeValue('channels', channels.value)
}

async function exportConfig() {
  let exportChannels = channels.value
  if (gatewayApiActive.value) {
    const remote = await listChannels(true)
    if (remote) {
      exportChannels = remote.map(fromDesktopChannel)
    }
  }
  const payload = {
    format: 'heigate-config',
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
      if (gatewayApiActive.value) {
        const remote = await listChannels()
        if (remote) {
          for (const channel of remote) {
            if (!(await deleteChannel(channel.id))) {
              showToast('清空失败，已有部分渠道未删除')
              return
            }
          }
        }
      }
      channels.value = []
      await clearValue('channels')
      showToast('本地工作区已清空')
    },
  }
}

// Model Latency & Speed Test helpers
const selectedModelNames = ref<string[]>([])

const isAllModelsSelected = computed(() => {
  const models = activeModelLatencyChannel.value?.modelNames || []
  if (models.length === 0) return false
  return models.every((m) => selectedModelNames.value.includes(m))
})

const isSomeModelsSelected = computed(() => {
  const models = activeModelLatencyChannel.value?.modelNames || []
  if (models.length === 0) return false
  const count = selectedModelNames.value.length
  return count > 0 && count < models.length
})

function toggleSelectAllModels() {
  const models = activeModelLatencyChannel.value?.modelNames || []
  if (isAllModelsSelected.value) {
    selectedModelNames.value = []
  } else {
    selectedModelNames.value = [...models]
  }
}

function toggleModelSelection(modelName: string) {
  const idx = selectedModelNames.value.indexOf(modelName)
  if (idx >= 0) {
    selectedModelNames.value.splice(idx, 1)
  } else {
    selectedModelNames.value.push(modelName)
  }
}

function handleModelCardClick(modelName: string) {
  const sel = window.getSelection()?.toString().trim()
  if (sel) return
  toggleModelSelection(modelName)
}

function openModelLatencyModal(channel: Channel) {
  activeModelLatencyChannel.value = channel
  selectedModelNames.value = []
  modelSearchQuery.value = ''
  if (channel.latency != null && channel.modelNames?.[0]) {
    const key = `${channel.gatewayId ?? channel.name}::${channel.modelNames[0]}`
    if (!modelLatencies.value[key]) {
      const isFailed = channel.status === 'offline' && Boolean(channel.lastError)
      modelLatencies.value[key] = {
        latency: channel.latency,
        ttft: channel.ttft ?? null,
        status: channel.status || 'healthy',
        error: isFailed ? channel.lastError : null,
        checkedAt: channel.lastCheck !== '尚未探活' ? channel.lastCheck : null,
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
  return `${channel.gatewayId ?? channel.name}::${modelName}`
}

function isModelFailed(channel: Channel, modelName: string): boolean {
  if (channel.probing) return false
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry) {
    if (entry.probing) return false
    return entry.status === 'error' || entry.status === 'failed' || entry.status === 'timeout'
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

function formatModelCheckedAt(value?: string | null): string {
  if (!value) return ''
  const checked = new Date(value)
  if (Number.isNaN(checked.getTime())) return ''
  const ageMs = Math.max(0, Date.now() - checked.getTime())
  if (ageMs < 60_000) return '刚刚'
  if (ageMs < 60 * 60_000) return `${Math.floor(ageMs / 60_000)} 分钟前`
  if (ageMs < 24 * 60 * 60_000) return `${Math.floor(ageMs / (60 * 60_000))} 小时前`
  return formatLogDateTime(value).slice(5, 16)
}

function getModelLatencyDisplay(channel: Channel, modelName: string): string {
  if (isModelProbing(channel, modelName)) return '—'
  if (isModelFailed(channel, modelName)) return '—'
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry?.ttft != null) return formatLatencySec(entry.ttft)
  if (entry?.latency != null) return formatLatencySec(entry.latency)
  // The channel-level latency is measured on its primary model. Showing it for
  // every model would invent a number for models that were never measured —
  // which is why a whole list used to display one identical value.
  if (isPrimaryModel(channel, modelName) && channel.latency != null && channel.status !== 'offline') {
    return formatLatencySec(channel.ttft ?? channel.latency)
  }
  return '—'
}

function getModelLatencyClass(channel: Channel, modelName: string): string {
  const key = getModelKey(channel, modelName)
  const entry = modelLatencies.value[key]
  if (entry?.probing || channel.probing) return 'probing'
  if (isModelFailed(channel, modelName)) {
    return 'timeout'
  }
  const inheritChannel = isPrimaryModel(channel, modelName) && channel.status !== 'offline'
  const lat = entry?.latency ?? (inheritChannel ? channel.latency : null)
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
    return `探活异常：${getModelErrorMessage(channel, modelName)} (点击查看详情与诊断)`
  }
  const checkedAt = entry?.checkedAt ? ` 最近检测：${formatModelCheckedAt(entry.checkedAt)}。` : ''
  if (entry?.ttft != null || entry?.latency != null) {
    return `模型调用成功，首字响应耗时 ${formatLatencySec(entry.ttft ?? entry.latency)}。${checkedAt}点击查看详情`
  }
  return '尚无该模型的探活记录，点击可查看状态或单独发送真实探活请求'
}

function isModelProbing(channel: Channel, modelName: string): boolean {
  if (channel.probing) return true
  return Boolean(modelLatencies.value[getModelKey(channel, modelName)]?.probing)
}

const filteredModalModels = computed(() => {
  const ch = activeModelLatencyChannel.value
  if (!ch) return []
  const list = ch.modelNames?.length ? [...ch.modelNames] : [ch.name]

  // The configured order, not a ranking: the primary model first, then exactly
  // as the channel lists them. Ranking by probe result made the rows rearrange
  // themselves mid-probe, moving the row the user was about to act on.
  const primaryIndex = list.findIndex((model) => isPrimaryModel(ch, model))
  if (primaryIndex > 0) {
    const [primary] = list.splice(primaryIndex, 1)
    list.unshift(primary)
  }
  return list
})

const modelLatencyRows = computed<ModelLatencyRow[]>(() => {
  const channel = activeModelLatencyChannel.value
  if (!channel) return []
  return filteredModalModels.value.map((name) => {
    const entry = modelLatencies.value[getModelKey(channel, name)]
    return {
      name,
      selected: selectedModelNames.value.includes(name),
      checkedAtLabel: entry?.checkedAt ? formatModelCheckedAt(entry.checkedAt) : '',
      latencyClass: getModelLatencyClass(channel, name),
      latencyDisplay: getModelLatencyDisplay(channel, name),
      latencyTooltip: getModelTooltip(channel, name),
      failed: isModelFailed(channel, name),
      injectionClass: getModelInjectionClass(channel, name),
      injectionTooltip: getModelInjectionTooltip(channel, name),
      injectionLabel: getModelInjectionLabel(channel, name),
      primary: isPrimaryModel(channel, name),
      probing: isModelProbing(channel, name),
      checkingInjection: isModelCheckingInjection(channel, name),
      copied: copiedModelId.value === name,
    }
  })
})

type ModelProbeOutcome = 'completed' | 'busy' | 'failed'

async function probeSingleModel(
  channel: Channel,
  modelName: string,
  options?: { silentBusy?: boolean },
): Promise<ModelProbeOutcome> {
  const key = getModelKey(channel, modelName)
  modelLatencies.value[key] = { ...(modelLatencies.value[key] || {}), probing: true }
  const t0 = performance.now()
  const isPrimary = isPrimaryModel(channel, modelName)
  try {
    if (gatewayApiActive.value && channel.gatewayId) {
      const probeRes = await probeChannelDetails(channel.gatewayId, modelName)
      if (probeRes === 'busy') {
        modelLatencies.value[key] = { ...(modelLatencies.value[key] || {}), probing: false, message: '渠道已有探活任务正在执行' }
        if (!options?.silentBusy) showToast(`渠道“${channel.name}”已有探活任务，请稍后重试`)
        return 'busy'
      }
      if (probeRes === 'disabled') {
        const errMsg = '渠道已停用，无法执行探活'
        modelLatencies.value[key] = { ...(modelLatencies.value[key] || {}), probing: false, status: 'error', error: errMsg, message: errMsg }
        return 'failed'
      }
      if (probeRes === null) {
        const errMsg = '探活请求失败，请检查网关状态'
        modelLatencies.value[key] = {
          ...(modelLatencies.value[key] || {}),
          probing: false,
          status: 'error',
          error: errMsg,
          message: errMsg,
        }
        return 'failed'
      }
      if (probeRes.length === 0) {
        // The server uses one in-flight slot per channel. Do not fall back to
        // a browser-side /models request here: that only tests list access and
        // can falsely mark an uncallable model as healthy.
        modelLatencies.value[key] = {
          ...(modelLatencies.value[key] || {}),
          probing: false,
          message: '渠道已有探活任务正在执行',
        }
        if (!options?.silentBusy) {
          showToast(`渠道“${channel.name}”已有探活任务，请稍后重试`)
        }
        return 'busy'
      }
      const res = probeRes?.[0]
      if (res) {
        const isOk = res.status === 'operational' || res.status === 'degraded' || res.status === 'healthy'
        const lat = res.latencyMs ?? (isOk ? Math.round(performance.now() - t0) : null)
        const hasErr = !isOk
        const errMsg = hasErr ? (res.message || '模型调用异常') : null
        modelLatencies.value[key] = {
          latency: lat,
          ttft: res.ttftMs ?? null,
          probing: false,
          status: res.status,
          error: errMsg,
          message: hasErr ? errMsg! : (res.status === 'degraded' ? '响应稍慢' : ''),
        }
        if (isPrimary) {
          channel.latency = lat
          channel.ttft = res.ttftMs ?? null
          if (isOk) {
            channel.status = res.status === 'degraded' ? 'degraded' : 'healthy'
            channel.lastError = ''
          } else {
            channel.status = 'offline'
            channel.lastError = errMsg!
          }
        }
        return 'completed'
      }
    }
    // A successful /models response does not prove that this key can perform
    // inference with this model. Without the local gateway we cannot run the
    // protocol-aware arithmetic challenge, so report the check as unavailable.
    const errMsg = '本地网关未连接，无法执行真实模型探活'
    modelLatencies.value[key] = {
      latency: null,
      ttft: null,
      probing: false,
      status: 'error',
      error: errMsg,
      message: errMsg,
      checkedAt: new Date().toISOString(),
    }
    if (isPrimary) {
      channel.latency = null
      channel.ttft = null
      channel.status = 'offline'
      channel.lastError = errMsg
    }
    return 'failed'
  } catch (err: any) {
    const errMsg = err?.message || '请求超时或网络异常'
    modelLatencies.value[key] = {
      latency: null,
      ttft: null,
      probing: false,
      status: 'timeout',
      error: errMsg,
      message: errMsg,
      checkedAt: new Date().toISOString(),
    }
    if (isPrimary) {
      channel.latency = null
      channel.ttft = null
      channel.status = 'offline'
      channel.lastError = errMsg
    }
    return 'failed'
  }
}

async function probeAllChannelModels(channel: Channel, requestedModels = channel.modelNames ?? []) {
  if (isProbingAllModels.value) return
  isProbingAllModels.value = true
  const models = requestedModels
  let completed = 0
  let failed = 0
  let interrupted = false

  try {
    for (const m of models) {
      const outcome = await probeSingleModel(channel, m, { silentBusy: true })
      if (outcome === 'busy') {
        interrupted = true
        break
      }
      if (outcome === 'completed') completed++
      if (outcome === 'failed') failed++
    }
    if (interrupted) {
      showToast(`渠道已有探活任务，本次完成 ${completed}/${models.length} 个模型`)
    } else {
      showToast(`探活完成 ${completed}/${models.length}${failed ? `，失败 ${failed}` : ''}`)
    }
  } catch (err: any) {
    showToast(`模型队列探活中断: ${err?.message || '未知异常'}`)
  } finally {
    isProbingAllModels.value = false
    await refreshChannels()
  }
}

function toChannelStatus(s?: string): 'healthy' | 'degraded' | 'offline' | 'unknown' {
  if (!s) return 'unknown'
  if (s === 'healthy' || s === 'operational') return 'healthy'
  if (s === 'degraded') return 'degraded'
  if (s === 'offline' || s === 'error' || s === 'failed' || s === 'timeout') return 'offline'
  return 'unknown'
}

async function batchDeleteSelectedModels(channel?: Channel | null) {
  if (!channel) return
  const toDelete = [...selectedModelNames.value]
  if (toDelete.length === 0) return

  const remaining = (channel.modelNames || []).filter((m) => !toDelete.includes(m))
  if (remaining.length === 0) {
    showToast('渠道至少需要保留一个模型，无法全部删除')
    return
  }

  const isDeletingPrimary = toDelete.includes(channel.primaryModel || '')

  confirmModal.value = {
    title: '批量删除模型确认',
    message: `确定要从渠道 “${channel.name}” 中批量删除已选的 ${toDelete.length} 个模型吗？`,
    description: isDeletingPrimary
      ? `注意：已选模型中包含当前主模型，删除后将自动将 “${remaining[0]}” 设为新的主模型。`
      : `删除后，网关将不再通过渠道 “${channel.name}” 分发这 ${toDelete.length} 个模型的请求。`,
    danger: true,
    confirmText: `确认删除 (${toDelete.length})`,
    onConfirm: async () => {
      channel.modelNames = remaining
      channel.models = remaining.length
      if (isDeletingPrimary) {
        channel.primaryModel = remaining[0]
        const newKey = getModelKey(channel, remaining[0])
        const cached = modelLatencies.value[newKey]
        if (cached) {
          channel.latency = cached.latency ?? null
          channel.ttft = cached.ttft ?? null
          if (cached.status) channel.status = toChannelStatus(cached.status)
          if (cached.error) channel.lastError = cached.error
        }
      }
      toDelete.forEach((m) => {
        delete modelLatencies.value[getModelKey(channel, m)]
      })
      selectedModelNames.value = []

      await persistChannel(channel)
      await refreshChannels()

      const fresh = channels.value.find((c) => (channel.gatewayId ? c.gatewayId === channel.gatewayId : c.name === channel.name))
      if (fresh) {
        activeModelLatencyChannel.value = fresh
      }

      showToast(`已成功从渠道“${channel.name}”删除 ${toDelete.length} 个模型`)
    },
  }
}

async function setAsPrimaryModel(channel?: Channel | null, modelName?: string) {
  if (!channel || !modelName) return
  if (channel.primaryModel === modelName) return

  channel.primaryModel = modelName
  const rest = (channel.modelNames || []).filter((m) => m !== modelName)
  channel.modelNames = [modelName, ...rest]

  const key = getModelKey(channel, modelName)
  const cached = modelLatencies.value[key]
  if (cached) {
    channel.latency = cached.latency ?? null
    channel.ttft = cached.ttft ?? null
    if (cached.status) channel.status = toChannelStatus(cached.status)
    if (cached.error) channel.lastError = cached.error
  }

  await persistChannel(channel)
  await refreshChannels()

  const fresh = channels.value.find((c) => (channel.gatewayId ? c.gatewayId === channel.gatewayId : c.name === channel.name))
  if (fresh) {
    activeModelLatencyChannel.value = fresh
  }

  showToast(`已将 “${modelName}” 设为主模型；如需验证，请手动探活`)
}

function setPrimaryFromRow(channel: Channel, modelName: string) {
  void setAsPrimaryModel(channel, modelName)
}

// ============================================================================
// Channel Probe Diagnostic & Error Inspection
// ============================================================================
const activeDiagnosticChannel = ref<Channel | null>(null)

function openChannelDiagnostic(c: Channel) {
  activeDiagnosticChannel.value = c
}
function copyDiagnosticRaw(rawError: string) {
  if (!rawError) return
  navigator.clipboard.writeText(rawError).then(() => {
    showToast('已复制完整报错数据到剪贴板')
  })
}

function openEditChannelFromDiag(c: Channel) {
  activeDiagnosticChannel.value = null
  openEditChannel(c)
}

async function retryProbeFromDiag(c: Channel) {
  await probeChannel(c)
  const updated = channels.value.find((ch) => ch.gatewayId === c.gatewayId || ch.name === c.name)
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
    const isValidFormat = parsed?.format === 'heigate-config'
    if (!isValidFormat || !Array.isArray(parsed.channels)) throw new Error('invalid config')
    const imported = parsed.channels.filter((channel: Channel) => channel?.name && channel?.url)
    if (gatewayApiActive.value) {
      const remote = await listChannels()
      const created: ApiChannel[] = []
      for (const channel of imported) {
        const saved = await createChannel({ ...toDesktopChannel(channel), apiKey: channel.apiKey ?? '' })
        if (!saved) {
          await Promise.all(created.map((channel) => deleteChannel(channel.id)))
          throw new Error('remote import failed')
        }
        created.push(saved)
      }
      if (remote) {
        for (const channel of remote) {
          if (!(await deleteChannel(channel.id))) {
            throw new Error('remote cleanup failed')
          }
        }
      }
      await refreshChannels()
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
    refreshChannels(),
    refreshGatewayHealth(),
    refreshSystemInfo(),
    refreshLogs(),
  ])
  showToast('已刷新网关状态与全部数据')
}

const selectedChannelId = ref<string | number | null>(null)

function selectChannel(c: Channel) {
  selectedChannelId.value = c.gatewayId ?? c.name
}

function getSelectedChannel(): Channel | null {
  if (selectedChannelId.value == null) return null
  return sortedChannels.value.find(c => (c.gatewayId != null && c.gatewayId === selectedChannelId.value) || c.name === selectedChannelId.value) || null
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
      channelToolbarRef.value?.focusSearch()
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
  }
}

onMounted(async () => {
  document.title = 'HeiGate'
  document.documentElement.classList.remove('dark')
  window.addEventListener('desktop-shortcut', onDesktopShortcut)
  checkTabsFit()
  window.addEventListener('resize', checkTabsFit)
  if (typeof ResizeObserver !== 'undefined' && navCenterRef.value) {
    navCenterObserver = new ResizeObserver(() => checkTabsFit())
    navCenterObserver.observe(navCenterRef.value)
  }
  const saved = await readValue<Channel[]>('channels')
  if (Array.isArray(saved)) channels.value = saved
  await refreshChannels()
  await loadCustomModelMappings()
  gatewayApiKey.value = (await getGatewayKey()) ?? ''
  await refreshGatewayHealth()
  await refreshSystemInfo()
  await refreshLogs()
  if (activeSection.value === 'analytics') {
    void loadAnalytics()
  }
  startLogsStream()
})

onBeforeUnmount(() => {
  stopLogsStream()
  window.removeEventListener('desktop-shortcut', onDesktopShortcut)
  window.removeEventListener('resize', checkTabsFit)
  if (navCenterObserver) {
    navCenterObserver.disconnect()
    navCenterObserver = null
  }
})

watch(channels, () => { if (!gatewayApiActive.value) void saveChannels() }, { deep: true })

async function refreshGatewayHealth() {
  const health = await checkGatewayHealth()
  gatewayOnline.value = health.ok
  gatewayLatency.value = health.latency
  if (health.version) appVersion.value = health.version
  gatewayLastChecked.value = health.ok ? `${formatLatencySec(health.latency)} 响应` : '无法连接'
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
    if (!ch.enabled) return
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

const activeModelProviderRows = computed<ModelProviderSpeedTestRow[]>(() => activeModelProviders.value.map((item) => ({
  channel: item.channel,
  upstreamModel: item.upstreamModel,
  protocolLabel: protocolShortLabel(item.channel.protocol),
  latencyDisplay: item.latencyDisplay,
  latencyClass: item.latencyClass,
  isProbing: item.isProbing,
  isFailed: item.isFailed,
  tooltip: item.tooltip,
  preferred: isFastestProvider(item),
})))

const activeModelFastestProvider = computed(() => getFastestProviderInfo(activeModelSpeedTestName.value))

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
    const entryLat = entry?.ttft != null ? entry.ttft : (entry?.latency != null ? entry.latency : null)
    const latMs = entryLat != null ? entryLat : (ch.modelNames?.[0] === upstream && ch.status !== 'offline' ? (ch.ttft ?? ch.latency) : null)
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
    if (!ch.enabled) return
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
    showToast(`探活完成：${target} 优选供应商为 “${fastest.channelName}” (首字 ${formatLatencySec(fastest.latency)})`)
  } else {
    showToast(`已完成 “${target}” 全部 ${matchingChannels.length} 家供应商探活 (可用: ${healthyCount}/${matchingChannels.length})`)
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

function openLogDetail(log: RequestLog) {
  activeLogDetail.value = log
}

function copyLogJson(log: RequestLog) {
  navigator.clipboard.writeText(JSON.stringify(log, null, 2)).then(() => {
    showToast('已复制完整日志 JSON 到剪贴板')
  })
}

function copyErrorMessage(msg: string) {
  navigator.clipboard.writeText(msg).then(() => {
    showToast('已复制报错信息到剪贴板')
  })
}

function getLogDiagnostic(log: RequestLog): {
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
      message: `上游渠道【${log.channel_name}】响应正常，全链路耗时 ${formatLatencySec(log.latency_ms)}，首字耗时 ${formatLatencySec(log.ttft_ms || log.latency_ms)}。`,
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

function startLogsStream() {
  if (logsUnsubscribe) return
  logsUnsubscribe = subscribeLogs({
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
    },
    onCleared: () => {
      desktopLogs.value = []
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
    const proxyRes = await apiFetchUpstreamModels(url.trim(), apiKey.trim(), protocol)
    if (proxyRes.ok && proxyRes.models && proxyRes.models.length > 0) {
      if (mode === 'new') {
        newChannelModelNames.value = proxyRes.models
        if (!newChannelPrimaryModel.value || !proxyRes.models.includes(newChannelPrimaryModel.value)) {
          newChannelPrimaryModel.value = proxyRes.models[0] || ''
        }
      } else {
        editChannelModelNames.value = proxyRes.models
        if (!editChannelPrimaryModel.value || !proxyRes.models.includes(editChannelPrimaryModel.value)) {
          editChannelPrimaryModel.value = proxyRes.models[0] || ''
        }
      }
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
      if (mode === 'new') {
        newChannelModelNames.value = fetchedModels
        if (!newChannelPrimaryModel.value || !fetchedModels.includes(newChannelPrimaryModel.value)) {
          newChannelPrimaryModel.value = fetchedModels[0] || ''
        }
      } else {
        editChannelModelNames.value = fetchedModels
        if (!editChannelPrimaryModel.value || !fetchedModels.includes(editChannelPrimaryModel.value)) {
          editChannelPrimaryModel.value = fetchedModels[0] || ''
        }
      }
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
  const primaryModel = newChannelPrimaryModel.value.trim()
  const modelNames = [...newChannelModelNames.value]
  if (primaryModel && !modelNames.includes(primaryModel)) {
    modelNames.unshift(primaryModel)
  }
  const model = primaryModel || modelNames[0] || ''
  if (!name || !rawUrl || !newChannelProtocol.value || !model) {
    showToast('请完整填写渠道名称、接口协议与主模型')
    return
  }
  const finalUrl = normalizeEndpointUrl(rawUrl)
  const draft: Channel = {
    name,
    url: finalUrl,
    apiKey: newChannelKey.value.trim(),
    protocol: newChannelProtocol.value,
    primaryModel: model,
    status: 'unknown',
    latency: null,
    ttft: null,
    models: modelNames.length,
    lastCheck: '尚未探活',
    accent: newChannelProtocol.value === 'anthropic' ? '#f59e0b' : newChannelProtocol.value === 'openai-responses' ? '#3b82f6' : '#10b981',
    enabled: newChannelEnabled.value,
    modelNames,
  }
  if (gatewayApiActive.value) {
    const created = await createChannel({ ...toDesktopChannel(draft), apiKey: newChannelKey.value.trim() })
    if (created) {
      channels.value.unshift(fromDesktopChannel(created))
      showToast(`渠道“${name}”创建成功`)
    } else {
      // 保留表单内容以便重试；不落本地存储，否则刷新后副本会消失。
      showToast(`渠道“${name}”创建失败，请检查网关状态后重试`)
      return
    }
  } else {
    channels.value.unshift(draft)
    showToast(`渠道“${name}”已保存`)
  }
  newChannelName.value = ''
  newChannelUrl.value = ''
  newChannelKey.value = ''
  showNewKey.value = false
  newChannelPrimaryModel.value = ''
  newChannelModelNames.value = []
  newChannelProtocol.value = 'openai-compatible'
  showAddChannel.value = false
  newChannelEnabled.value = true
  activeSection.value = 'channels'
}

function openAddChannel() {
  showNewKey.value = false
  showAddChannel.value = true
}

let isRefreshingLogs = false
async function refreshLogs() {
  if (isRefreshingLogs || !gatewayApiActive.value) return
  isRefreshingLogs = true
  try {
    const logs = await listLogs(100)
    if (logs) {
      desktopLogs.value = logs
    }
  } finally {
    isRefreshingLogs = false
  }
}

async function refreshSystemInfo() {
  if (gatewayApiActive.value) {
    const info = await getSystemInfo()
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
  if (gatewayApiActive.value) {
    const ok = await updateSettings({ routing_strategy: strategy })
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
  if (gatewayApiActive.value) {
    const ok = await updateSettings({ log_retention_days: days })
    if (ok) {
      showToast(days > 0 ? `日志保留时长已设为 ${days} 天` : '已设为永久保留日志')
      await refreshSystemInfo()
    }
  }
}

async function handleSetLogMaxCount(count: number) {
  logMaxCount.value = count
  if (gatewayApiActive.value) {
    const ok = await updateSettings({ log_max_count: count })
    if (ok) {
      showToast(count > 0 ? `日志容量上限已设为 ${count.toLocaleString()} 条` : '已设为不限制流水条数')
      await refreshSystemInfo()
    }
  }
}

async function handlePruneLogs() {
  isPruningLogs.value = true
  try {
    const res = await pruneLogs()
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
        const ok = await clearLogs()
        if (ok) {
          desktopLogs.value = []
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

function formatDurationPill(ms?: number): string {
  if (ms === undefined || ms === null || ms < 0) return '0 s'
  return formatLatencySec(ms)
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
    const key = gatewayApiKey.value || await getGatewayKey()
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

let backdropMouseDownPos = { x: 0, y: 0 }

// A modal backdrop (z-index ≥ 999) covers the top navigation bar, which is the
// window's normal drag region. While one is open we expose a thin strip across
// the very top of the window so the title bar stays a drag handle — matching
// the native expectation that you can always move a window by its top edge.
const hasOpenModal = computed(() => openModalCount.value > 0)

function onBackdropMouseDown(event: MouseEvent) {
  if (event.button !== 0) return
  if (event.target !== event.currentTarget) return
  backdropMouseDownPos = { x: event.screenX, y: event.screenY }
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
  const hasSelection = Boolean(window.getSelection()?.toString().trim())
  const isEditable = target && (
    target.tagName === 'INPUT' ||
    target.tagName === 'TEXTAREA' ||
    target.isContentEditable ||
    Boolean(target.closest('input, textarea, [contenteditable="true"]'))
  )
  if (!isEditable && !hasSelection) {
    event.preventDefault()
  }
}
</script>

<template>
  <div class="m26-window" @contextmenu="handleContextMenu">
    <!-- Title-bar drag strip: only interactive while a modal covers the nav
         bar, so it never steals clicks from the bar itself. -->
    <div
      v-if="hasOpenModal"
      class="m26-titlebar-drag"
      data-tauri-drag-region="deep"
      aria-hidden="true"
    ></div>

    <!-- Background Ambient Glow Canvas -->
    <div class="m26-ambient-layer">
      <div class="m26-glow-orb orb-1"></div>
      <div class="m26-glow-orb orb-2"></div>
      <div class="m26-glow-orb orb-3"></div>
    </div>

    <!-- TOP FLOATING PILL CAPSULE NAVIGATION BAR (macOS 26 White) -->
    <header class="m26-top-navbar" data-tauri-drag-region="deep">
      <!-- Left: macOS window buttons spacer -->
      <div class="m26-nav-left">
        <div class="m26-traffic-lights" v-if="!isDesktop" aria-hidden="true">
          <span class="light red"></span>
          <span class="light yellow"></span>
          <span class="light green"></span>
        </div>
        <div class="m26-native-titlebar-spacer" v-else></div>
      </div>

      <!-- Center: FLOATING PILL CAPSULE NAV (Flex centered & Auto collapse text when constrained) -->
      <div class="m26-nav-center" ref="navCenterRef">
        <nav class="m26-floating-nav" :class="{ 'compact-tabs': isCompactTabs }">
          <!-- 1. 看板 -->
          <button
            class="m26-pill-item"
            :class="{ active: activeSection === 'analytics' }"
            @click="activeSection = 'analytics'"
            title="数据看板"
          >
            <svg class="m26-pill-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor">
              <path d="M18 20V10" />
              <path d="M12 20V4" />
              <path d="M6 20v-6" />
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
              <rect x="2" y="3" width="20" height="7" rx="2" ry="2" />
              <rect x="2" y="14" width="20" height="7" rx="2" ry="2" />
              <line x1="6" y1="6.5" x2="6.01" y2="6.5" />
              <line x1="6" y1="17.5" x2="6.01" y2="17.5" />
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
              <polygon points="12 2 2 7 12 12 22 7 12 2" />
              <polyline points="2 17 12 22 22 17" />
              <polyline points="2 12 12 17 22 12" />
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
              <circle cx="12" cy="12" r="10" />
              <polyline points="12 6 12 12 16 14" />
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
              <line x1="4" y1="21" x2="4" y2="14" />
              <line x1="4" y1="10" x2="4" y2="3" />
              <line x1="12" y1="21" x2="12" y2="12" />
              <line x1="12" y1="8" x2="12" y2="3" />
              <line x1="20" y1="21" x2="20" y2="16" />
              <line x1="20" y1="12" x2="20" y2="3" />
              <line x1="1" y1="14" x2="7" y2="14" />
              <line x1="9" y1="8" x2="15" y2="8" />
              <line x1="17" y1="16" x2="23" y2="16" />
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
        <!-- SECTION: DATA DASHBOARD (KANBAN) -->
        <section v-if="activeSection === 'analytics'" class="m26-section-view">
          <AnalyticsOverviewMetrics
            :healthy-channels="healthyChannels"
            :channel-count="channels.length"
            :enabled-channels-count="enabledChannels.length"
            :average-latency="averageLatency"
            :routing-strategy="routingStrategy"
            :model-count="models.length"
            :available-models="availableModels"
          />

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
                >
今日
</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 7 }"
                  @click="setAnalyticsDays(7)"
                >
近 7 天
</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 14 }"
                  @click="setAnalyticsDays(14)"
                >
近 14 天
</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 30 }"
                  @click="setAnalyticsDays(30)"
                >
近 30 天
</button>
                <button
                  class="m26-time-pill-btn"
                  :class="{ active: analyticsDays === 0 }"
                  @click="setAnalyticsDays(0)"
                >
全部历史
</button>
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
                  <path d="M23 4v6h-6M1 20v-6h6" />
                  <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
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
                    <path d="M18 20V10M12 20V4M6 20v-6" />
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
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
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
                    <circle cx="12" cy="12" r="10" />
                    <polyline points="12 6 12 12 16 14" />
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
                    <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
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
                    <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
                    <polyline points="22 4 12 14.01 9 11.01" />
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

          <AnalyticsTrendCharts
            :analytics-data="analyticsData"
            :max-daily-requests="maxDailyRequests"
            :max-daily-tokens="maxDailyTokens"
          />

          <AnalyticsModelBreakdown
            :models="filteredAnalyticsModels"
            :total-requests="analyticsData?.summary?.total_requests ?? 0"
            :search="analyticsModelSearch"
            @update:search="analyticsModelSearch = $event"
          />

          <AnalyticsChannelPerformance :channels="analyticsData?.channels ?? []" />
        </section>

        <!-- SECTION 2: CHANNELS -->
        <section v-if="activeSection === 'channels'" class="m26-section-view">
          <!-- Toolbar: Search, Filter Pills, View Mode Switcher, Batch Probe, Add Channel -->
          <ChannelToolbar
            ref="channelToolbarRef"
            data-tauri-drag-region="deep"
            :search="channelSearch"
            :filter-status="channelStatusFilter"
            :view-mode="channelViewMode"
            :total-count="channels.length"
            :healthy-count="healthyChannelsCount"
            :degraded-count="degradedChannelsCount"
            :error-count="errorChannelsCount"
            :disabled-count="disabledChannelsCount"
            @update:search="channelSearch = $event"
            @update:filter-status="channelStatusFilter = $event"
            @update:view-mode="setChannelViewMode($event)"
            @add-channel="openAddChannel"
          />

          <!-- Card Grid View -->
          <ChannelGridView
            v-if="channelViewMode === 'grid'"
            :channels="sortedChannels"
            :selected-channel="getSelectedChannel()"
            :search-query="channelSearch"
            @select="selectChannel($event as Channel)"
            @toggle="toggleChannel($event as Channel)"
            @probe="probeChannel($event as Channel)"
            @edit="openEditChannel($event as Channel)"
            @duplicate="duplicateChannel($event as Channel)"
            @remove="removeChannel($event as Channel)"
            @open-models="openModelLatencyModal($event as Channel)"
            @open-diagnostic="openChannelDiagnostic($event as Channel)"
            @open-inject="openInjectionModal($event as Channel)"
            @add-channel="openAddChannel"
            @clear-search="channelSearch = ''; channelStatusFilter = 'all'"
          />

          <!-- High-Density Table View -->
          <ChannelTableView
            v-else
            :channels="sortedChannels"
            :selected-channel="getSelectedChannel()"
            :search-query="channelSearch"
            @select="selectChannel($event as Channel)"
            @toggle="toggleChannel($event as Channel)"
            @probe="probeChannel($event as Channel)"
            @edit="openEditChannel($event as Channel)"
            @duplicate="duplicateChannel($event as Channel)"
            @remove="removeChannel($event as Channel)"
            @open-models="openModelLatencyModal($event as Channel)"
            @open-diagnostic="openChannelDiagnostic($event as Channel)"
            @open-inject="openInjectionModal($event as Channel)"
            @add-channel="openAddChannel"
            @clear-search="channelSearch = ''; channelStatusFilter = 'all'"
          />
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
              <button
                class="m26-action-btn secondary"
                @click="openCustomMappingsModal"
                title="自定义模型归类与别名合并设置"
              >
                <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M4 6h16M4 12h10M4 18h14" />
                </svg>
                <span>模型归类设置</span>
                <span v-if="Object.keys(customModelMappings).length > 0" class="m26-badge-count">{{ Object.keys(customModelMappings).length }}</span>
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
                    {{ m.status === 'healthy' ? m.latency : m.status === 'degraded' ? `${m.latency} 波动` : m.status === 'unknown' ? '待探活' : '离线' }}
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
                      <circle cx="18" cy="18" r="3" />
                      <circle cx="6" cy="6" r="3" />
                      <path d="M13 6h3a2 2 0 0 1 2 2v7" />
                      <line x1="6" y1="9" x2="6" y2="21" />
                    </svg>
                    <span>{{ m.aliases.length }} 个上游别名</span>
                  </span>
                  <span class="m26-aliases-text truncate font-mono">
                    {{ m.aliases.slice(0, 2).join(', ') }}{{ m.aliases.length > 2 ? ` 等 +${m.aliases.length - 2}` : '' }}
                  </span>
                  <span class="m26-aliases-more-btn">
                    <span>查看全部</span>
                    <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M6 12l4-4-4-4" />
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
                      <polyline points="20 6 9 17 4 12" />
                    </svg>
                    <span>标准同名直通</span>
                  </span>
                  <span class="m26-aliases-text">各渠道模型名一致</span>
                  <span class="m26-aliases-more-btn">
                    <span>明细</span>
                    <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M6 12l4-4-4-4" />
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
                    :title="`${ch.name} · ${ch.status === 'healthy' ? '就绪' : ch.status === 'degraded' ? '延迟较高' : '离线'}${ch.latency ? ' · ' + formatLatencySec(ch.latency) : ''}${ch.upstreamModel ? ' · 上游: ' + ch.upstreamModel : ''}`"
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
                    v-if="m.name !== 'Auto'"
                    class="m26-model-test-btn"
                    @click="openModelSpeedTestFor(m.name)"
                    title="在全部支持此模型的渠道中发起并发探活"
                  >
                    <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2">
                      <polygon points="8 2 14 5 14 11 8 14 2 11 2 5" />
                    </svg>
                    <span>探活此模型</span>
                  </button>
                  <button
                    v-if="m.name !== 'Auto'"
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
        <LogsView
          v-if="activeSection === 'logs'"
          :logs="filteredLogs"
          :total-logs="desktopLogs.length"
          :search="channelSearch"
          :stream-connected="logsStreamConnected"
          :auto-scroll="autoScrollLogs"
          :clearing="isClearingLogs"
          @update:search="channelSearch = $event"
          @update:auto-scroll="autoScrollLogs = $event"
          @clear="handleClearLogs"
          @open-detail="openLogDetail"
        />
        <section v-if="false" class="m26-section-view">
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
                      <span class="m26-pill-total" :title="'总响应耗时: ' + formatLatencySec(log.latency_ms)">
                        {{ formatDurationPill(log.latency_ms) }}
                      </span>
                      <span class="m26-pill-ttft" :title="'首字耗时 (TTFT): ' + formatLatencySec(log.ttft_ms || log.latency_ms)">
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
            <RoutingSettings :routing-strategy="routingStrategy" @select="setRoutingStrategy" />

            <GatewayAccessSettings
              :gateway-base-url="gatewayBaseUrl"
              :gateway-api-key="gatewayApiKey"
              :masked-overview-api-key="maskedOverviewApiKey"
              v-model:show-api-key="showApiKey"
              @copy-url="copyGatewayUrl"
              @copy-key="copyApiKey"
            />

            <StorageSettings
              :system-info="systemInfo"
              :fallback-logs-count="desktopLogs.length"
              :is-pruning-logs="isPruningLogs"
              :log-retention-days="logRetentionDays"
              :log-max-count="logMaxCount"
              @prune-logs="handlePruneLogs"
              @copy-path="copySystemPath($event, '数据库')"
              @set-retention="handleSetLogRetention"
              @set-max-count="handleSetLogMaxCount"
              @export-config="exportConfig"
              @import-config="openImport"
              @clear-workspace="clearWorkspace"
            />

            <ModelRulesSettings
              v-model:new-mapping-source="newMappingSource"
              v-model:new-mapping-target="newMappingTarget"
              :all-detected-channel-models="allDetectedChannelModels"
              :existing-canonical-model-names="existingCanonicalModelNames"
              :custom-model-mappings="customModelMappings"
              :app-version="appVersion"
              @add-mapping="handleAddSettingsMapping"
              @remove-mapping="handleRemoveCustomMapping"
            />
          </div>
        </section>
      </div>
    </main>

    <!-- Hidden Input for Import -->
    <input ref="configInput" type="file" accept=".json" class="hidden" @change="importConfig" />

    <!-- MODAL: INJECTION CHECK (Redesigned) -->
    <div v-if="showInjectionModal" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-1" class="m26-modal-backdrop m26-inject-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showInjectionModal = false)">
      <ChannelInjectionContent
        :channel="injectionChannel"
        :results="injectionResults"
        :checking="isCheckingInjection"
        :copied-model-id="copiedModelId"
        :copied-injection-text="copiedInjectionText"
        @copy-model="copyModelId"
        @copy-content="copyInjectionContent"
        @check-primary="runInjectionCheck(false)"
        @check-all="runInjectionCheck(true)"
        @close="showInjectionModal = false"
      />
    </div>

    <!-- MODAL: ADD CHANNEL -->
    <div v-if="showAddChannel" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-2" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showAddChannel = false)">
      <div class="m26-modal-sheet">
        <div class="m26-modal-header">
          <h3 id="m26-modal-title-2" class="m26-modal-title">添加供应商渠道</h3>
          <button class="m26-close-btn" aria-label="关闭" @click="showAddChannel = false">✕</button>
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
            <ChannelModelManager
              v-model:primary-model="newChannelPrimaryModel"
              v-model:model-names="newChannelModelNames"
              :is-fetching="isFetchingNewModels"
              @fetch-upstream="fetchUpstreamModels('new')"
            />
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="showAddChannel = false">取消</button>
          <button class="m26-action-btn primary" @click="addChannel">确认创建</button>
        </div>
      </div>
    </div>

    <!-- MODAL: EDIT CHANNEL -->
    <div v-if="showEditChannel" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-3" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => showEditChannel = false)">
      <div class="m26-modal-sheet">
        <div class="m26-modal-header">
          <h3 id="m26-modal-title-3" class="m26-modal-title">编辑渠道配置</h3>
          <button class="m26-close-btn" aria-label="关闭" @click="showEditChannel = false">✕</button>
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
            <ChannelModelManager
              v-model:primary-model="editChannelPrimaryModel"
              v-model:model-names="editChannelModelNames"
              :is-fetching="isFetchingEditModels"
              @fetch-upstream="fetchUpstreamModels('edit')"
            />
          </div>
        </div>

        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="showEditChannel = false">取消</button>
          <button class="m26-action-btn primary" @click="saveEditChannel">保存修改</button>
        </div>
      </div>
    </div>

    <!-- MODAL: CONFIRM ACTION -->
    <div v-if="confirmModal" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-4" class="m26-modal-backdrop m26-confirm-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => confirmModal = null)">
      <div class="m26-modal-sheet m26-confirm-sheet">
        <div class="m26-modal-header">
          <h3 id="m26-modal-title-4" class="m26-modal-title">{{ confirmModal.title }}</h3>
          <button class="m26-close-btn" aria-label="关闭" @click="confirmModal = null">✕</button>
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
    <div v-if="activeModelLatencyChannel" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-5" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => { activeModelLatencyChannel = null; selectedModelNames = [] })">
      <div class="m26-modal-sheet m26-models-sheet">
        <div class="m26-modal-header">
          <div class="m26-modal-heading">
            <!-- The channel name is the title: it is what identifies this modal,
                 and a generic "模型管理" heading only repeats the button that
                 opened it. -->
            <h3
              id="m26-modal-title-5"
              class="m26-modal-title m26-modal-channel"
              :title="activeModelLatencyChannel.url"
            >
              {{ activeModelLatencyChannel.name }}
            </h3>
            <span class="m26-proto-pill" :class="activeModelLatencyChannel.protocol">
              {{ protocolShortLabel(activeModelLatencyChannel.protocol) }}
            </span>
            <span class="m26-modal-count">
              · {{ activeModelLatencyChannel.modelNames?.length || 1 }} 个模型
            </span>
          </div>
          <button class="m26-close-btn" aria-label="关闭" @click="activeModelLatencyChannel = null; selectedModelNames = []">✕</button>
        </div>

        <ChannelModelLatencyContent
          :channel="activeModelLatencyChannel"
          :rows="modelLatencyRows"
          :selected-count="selectedModelNames.length"
          :all-selected="isAllModelsSelected"
          :some-selected="isSomeModelsSelected"
          :probing-all="isProbingAllModels"
          :checking-all-injections="isCheckingAllInjections"
          @select-all="toggleSelectAllModels"
          @delete-selected="batchDeleteSelectedModels(activeModelLatencyChannel)"
          @batch-action="requestBatchModelAction(activeModelLatencyChannel, $event)"
          @toggle-selection="toggleModelSelection"
          @card-click="handleModelCardClick"
          @copy-model="copyModelId"
          @open-model-detail="openModelErrorDetail(activeModelLatencyChannel, $event)"
          @open-injection-detail="openModelInjectionDetail(activeModelLatencyChannel, $event)"
          @set-primary="setPrimaryFromRow(activeModelLatencyChannel, $event)"
          @probe-model="probeSingleModel(activeModelLatencyChannel, $event)"
          @check-injection="checkSingleModelInjection(activeModelLatencyChannel, $event)"
        />
      </div>
    </div>

    <!-- MODAL: MODEL MULTI-PROVIDER SPEED TEST (测试此模型所有供应商) -->
    <div
      v-if="activeModelSpeedTestName"
      v-focus-trap role="dialog" aria-modal="true" aria-label="模型测速" class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, closeModelSpeedTestModal)"
    >
      <ChannelProviderSpeedTestContent
        v-if="activeModelSpeedTestName"
        :model-name="activeModelSpeedTestName"
        :model-family="getModelFamily(activeModelSpeedTestName)"
        :providers="activeModelProviderRows"
        :query="modelSpeedTestQuery"
        :fastest="activeModelFastestProvider"
        :probing-all="isProbingModelProviders"
        :copied="copiedModelId === activeModelSpeedTestName"
        @update:query="modelSpeedTestQuery = $event"
        @copy-model="copyModelId(activeModelSpeedTestName)"
        @probe-all="probeAllProvidersForModel(activeModelSpeedTestName)"
        @probe-provider="probeSingleModel($event.channel, $event.upstreamModel)"
        @open-diagnostic="openModelErrorDetail($event.channel, $event.upstreamModel)"
        @close="closeModelSpeedTestModal"
      />
    </div>

    <!-- MODAL: MODEL ALIASES DETAIL -->
    <div
      v-if="activeAliasModalModel"
      v-focus-trap role="dialog" aria-modal="true" aria-label="模型别名" class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, closeAliasModal)"
    >
      <ChannelModelAliasContent
        v-if="activeAliasModalModel"
        :model="activeAliasModalModel"
        :family="getModelFamily(activeAliasModalModel.name)"
        :aliases="filteredModalAliases"
        :channel-details="filteredModalChannelDetails"
        :search="aliasModalSearch"
        :copied-text="copiedAliasText"
        :mapped-aliases="customModelMappings"
        @update:search="aliasModalSearch = $event"
        @copy-model="copyModelId(activeAliasModalModel.name)"
        @copy-aliases="copyAllAliases(activeAliasModalModel)"
        @copy-alias="copyAlias"
        @remove-mapping="handleRemoveCustomMapping"
        @open-speed-test="openModelSpeedTestFor(activeAliasModalModel.name)"
        @open-classify="openClassifyDialog('', activeAliasModalModel.name)"
        @close="closeAliasModal"
      />
    </div>

    <!-- MODAL: QUICK MODEL CLASSIFY -->
    <div
      v-if="showClassifyModal"
      v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-6" class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, () => showClassifyModal = false)"
    >
      <ModelMappingContent
        v-if="showClassifyModal"
        mode="classify"
        :source-suggestions="allDetectedChannelModels"
        :target-options="existingCanonicalModelNames"
        :initial-source="classifySourceModel"
        :initial-target="classifyTargetModel"
        @submit="submitClassify"
        @close="showClassifyModal = false"
      />
    </div>

    <!-- MODAL: CUSTOM MODEL MAPPINGS MANAGER -->
    <div
      v-if="showCustomMappingsModal"
      v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-7" class="m26-modal-backdrop"
      @mousedown="onBackdropMouseDown"
      @click="handleBackdropClick($event, () => showCustomMappingsModal = false)"
    >
      <ModelMappingContent
        v-if="showCustomMappingsModal"
        mode="manage"
        :source-suggestions="allDetectedChannelModels"
        :target-options="existingCanonicalModelNames"
        :mappings="customModelMappings"
        @submit="handleAddManagerMapping"
        @remove="handleRemoveCustomMapping"
        @close="showCustomMappingsModal = false"
      />
    </div>

    <!-- MODAL: CHANNEL PROBE & ERROR DIAGNOSIS -->
    <div v-if="activeDiagnosticChannel" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-8" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => activeDiagnosticChannel = null)">
      <div class="m26-modal-sheet m26-diagnostic-sheet">
        <div class="m26-modal-header">
          <div class="flex items-center gap-2">
            <div class="m26-diag-icon-badge danger">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 9v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
            </div>
            <div>
              <h3 id="m26-modal-title-8" class="m26-modal-title">探活诊断与报错详情</h3>
              <p class="m26-modal-subtitle">
                渠道 “{{ activeDiagnosticChannel.name }}” · {{ activeDiagnosticChannel.url }}
              </p>
            </div>
          </div>
          <button class="m26-close-btn" aria-label="关闭" @click="activeDiagnosticChannel = null">✕</button>
        </div>

        <ChannelDiagnosticContent
          :channel="activeDiagnosticChannel"
          @copy-raw="copyDiagnosticRaw"
        />

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
    <div v-if="activeLogDetail" v-focus-trap role="dialog" aria-modal="true" aria-labelledby="m26-modal-title-9" class="m26-modal-backdrop" @mousedown="onBackdropMouseDown" @click="handleBackdropClick($event, () => activeLogDetail = null)">
      <div class="m26-modal-sheet m26-log-detail-sheet">
        <div class="m26-modal-header">
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
              <h3 id="m26-modal-title-9" class="m26-modal-title">请求流水详情 #{{ activeLogDetail.id }}</h3>
              <p class="m26-modal-subtitle">
                {{ formatLogTime(activeLogDetail.created_at) }} · 网关全链路诊断
              </p>
            </div>
          </div>
          <button class="m26-close-btn" aria-label="关闭" @click="activeLogDetail = null">✕</button>
        </div>

        <LogDetailContent
          :log="activeLogDetail"
          :diagnostic="getLogDiagnostic(activeLogDetail)"
          @copy-log-json="copyLogJson"
          @copy-error-message="copyErrorMessage"
        />
        <div class="m26-modal-footer">
          <button class="m26-glass-btn" @click="copyLogJson(activeLogDetail)">复制 JSON</button>
          <button class="m26-action-btn primary" @click="activeLogDetail = null">关闭</button>
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

<style>
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
.m26-seg-btn,
.m26-card-switch,
.m26-card-btn,
.m26-action-btn,
.m26-glass-btn,
.m26-close-btn,
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
  height: 78px;
  min-height: 78px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  /* The top padding clears the traffic lights, which float over the content
     because the window uses an overlay title bar. */
  padding: 26px 24px 6px 24px;
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

/* Mini Grid in Overview */

/* ==========================================================================
   Section 2: Channels Management
   ========================================================================== */

/* Channel Card Grid */

/* Modern macOS 26 White Channel Card */

/* Card Header */

/* Card Switch */

/* Service / Channel Tag Row */

/* Card Models List - 1 row per model, left name, right latency / error */

/* Latency Badge */

/* Error Badge */

/* More / Expand Button */

/* Card Action Buttons (探活, 编辑, 复制, 删除) - pinned to bottom */
.m26-proto-pill {
  font-size: 10px;
  font-weight: 600;
  padding: 2px 7px;
  border-radius: 5px;
  white-space: nowrap;
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
.m26-inject-pill {
  font-size: 10px;
  font-weight: 700;
  padding: 2px 8px;
  border-radius: 5px;
  border: none;
  cursor: pointer;
  white-space: nowrap;
  font-family: inherit;
}
.m26-inject-pill.unknown {
  background: rgba(100, 116, 139, 0.12);
  color: #64748b;
}
.m26-inject-pill.clean {
  background: rgba(5, 150, 105, 0.12);
  color: #059669;
}
.m26-inject-pill.suspected {
  background: rgba(217, 119, 6, 0.14);
  color: #b45309;
}
.m26-inject-pill.detected {
  background: rgba(220, 38, 38, 0.12);
  color: #dc2626;
}
.m26-inject-pill.error {
  background: rgba(100, 116, 139, 0.12);
  color: #94a3b8;
}
.m26-modal-sheet.m26-inject-sheet {
  width: 860px !important;
  max-width: min(920px, 92vw) !important;
  max-height: 88vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-modal-body.m26-inject-body {
  flex: 1;
  overflow-y: auto;
  padding: 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  max-height: calc(88vh - 130px);
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-summary {
  display: flex;
  align-items: center;
  gap: 10px;
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-meta {
  font-size: 12px;
  color: var(--m26-text-secondary);
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-desc-box {
  background: rgba(0, 0, 0, 0.02);
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 8px;
  padding: 10px 14px;
}

.m26-inject-desc {
  font-size: 12px;
  line-height: 1.6;
  color: var(--m26-text-secondary);
  margin: 0;
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
  /* NO internal scrollbar - expands naturally inside m26-inject-body */
  overflow: visible !important;
  max-height: none !important;
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-row {
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 10px;
  padding: 12px 14px;
  background: #ffffff;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-row-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.m26-inject-model {
  font-size: 13px;
  font-weight: 600;
  font-family: ui-monospace, SFMono-Regular, monospace;
  color: var(--m26-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text !important;
  -webkit-user-select: text !important;
  cursor: text;
}

.m26-tokens-pill {
  font-size: 11px;
  font-family: ui-monospace, monospace;
  color: var(--m26-text-secondary);
  background: rgba(0, 0, 0, 0.04);
  padding: 2px 7px;
  border-radius: 5px;
  border: 1px solid rgba(0, 0, 0, 0.05);
}

.m26-inject-row-note {
  font-size: 12px;
  color: var(--m26-text-secondary);
  margin-top: 8px;
  padding: 6px 10px;
  background: rgba(0, 0, 0, 0.02);
  border-radius: 6px;
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-content-container {
  margin-top: 10px;
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 8px;
  overflow: hidden;
  background: #f8fafc;
}

.m26-inject-content-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  background: rgba(0, 0, 0, 0.03);
  border-bottom: 1px solid rgba(0, 0, 0, 0.06);
}

.m26-inject-content-title {
  font-size: 11.5px;
  font-weight: 600;
  color: var(--m26-text-secondary);
  user-select: text !important;
  -webkit-user-select: text !important;
}

.m26-inject-content {
  /* NO internal scrollbar - expands naturally so user scrolls entire modal with ONE single scrollbar */
  overflow: visible !important;
  max-height: none !important;
  background: transparent;
  border: none;
  border-radius: 0;
  padding: 12px 14px;
  font-size: 12.5px;
  line-height: 1.65;
  white-space: pre-wrap;
  word-break: break-word;
  margin: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  color: var(--m26-text);
  user-select: text !important;
  -webkit-user-select: text !important;
  cursor: text;
}

.m26-inject-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 48px 20px;
  color: var(--m26-text-muted);
  font-size: 13px;
  text-align: center;
}

.m26-empty-shield-icon {
  opacity: 0.35;
  color: var(--m26-accent);
}

.m26-inject-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 22px;
  background: #f8fafc;
  border-top: 1px solid var(--m26-border);
  flex-shrink: 0;
}

.m26-inject-footer-left {
  display: flex;
  align-items: center;
  font-size: 12px;
  color: var(--m26-text-secondary);
  min-width: 0;
}

.m26-inject-tip {
  font-size: 11.5px;
  color: var(--m26-text-muted);
}

.m26-inject-running-state {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--m26-accent);
  font-weight: 500;
  font-size: 12px;
}

.m26-inject-footer-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.m26-inject-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  font-size: 12.5px;
  font-weight: 500;
  padding: 7px 16px;
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.16s cubic-bezier(0.16, 1, 0.3, 1);
  user-select: none;
}

.m26-inject-btn.secondary {
  background: #ffffff;
  color: var(--m26-text);
  border: 1px solid var(--m26-border-bright);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
}

.m26-inject-btn.secondary:hover:not(:disabled) {
  border-color: var(--m26-accent);
  color: var(--m26-accent);
  box-shadow: 0 2px 6px rgba(79, 70, 229, 0.1);
}

.m26-inject-btn.primary {
  background: linear-gradient(135deg, #4f46e5 0%, #4338ca 100%);
  color: #ffffff;
  border: none;
  font-weight: 600;
  box-shadow: 0 2px 8px rgba(79, 70, 229, 0.28);
}

.m26-inject-btn.primary:hover:not(:disabled) {
  background: linear-gradient(135deg, #4338ca 0%, #3730a3 100%);
  box-shadow: 0 4px 12px rgba(79, 70, 229, 0.36);
  transform: translateY(-1px);
}

.m26-inject-btn:active:not(:disabled) {
  transform: scale(0.98);
}

.m26-inject-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
  transform: none;
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
  min-width: 880px;
  border-collapse: collapse;
  text-align: left;
  font-size: 13px;
  table-layout: fixed;
}
.m26-table th {
  background: #f8fafc;
  padding: 10px 10px;
  font-size: 11px;
  font-weight: 600;
  color: var(--m26-text-muted);
  border-bottom: 1px solid var(--m26-border);
  white-space: nowrap;
}
.m26-table td {
  padding: 10px 10px;
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

.m26-model-status-chip.unknown {
  background: rgba(100, 116, 139, 0.08);
  color: #64748b;
  border: 1px solid rgba(100, 116, 139, 0.16);
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

.m26-status-indicator-dot.unknown {
  background: #94a3b8;
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
.m26-model-item-info > div:hover .m26-model-row-copy,
.m26-model-row-copy:focus-visible {
  opacity: 1;
}
.m26-model-row-copy {
  width: 22px;
  height: 22px;
  justify-content: center;
  padding: 0;
  overflow: hidden;
  opacity: 0;
  background: transparent;
  border-color: transparent;
}
.m26-model-row-copy span {
  display: none;
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

.m26-pref-group-icon.icon-version {
  background: rgba(20, 184, 166, 0.1);
  color: #0f766e;
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

.m26-route-type-badge.smart_quality {
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
/* Transparent drag handle pinned to the window's top edge; mounted only while a
   modal is open (see hasOpenModal). Sits above every backdrop — base 999, the
   injection backdrop 1100, the confirm backdrop 1200 — so the title bar is
   draggable whichever dialog is showing. The height matches the macOS title
   bar; native traffic lights render on their own layer above it and keep
   working. */
.m26-titlebar-drag {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  height: 28px;
  z-index: 1300;
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
  user-select: text;
  -webkit-user-select: text;
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
  user-select: text;
  -webkit-user-select: text;
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
.m26-inject-backdrop {
  z-index: 1100;
}

/* ==========================================================================
   Model Latency & Speed Test Sheet
   ========================================================================== */
.m26-models-sheet {
  max-width: 860px;
  max-height: 85vh;
}
/* This modal is a list; the header only needs to identify the channel, so it
   carries less vertical padding than the default modal chrome. */
.m26-models-sheet .m26-modal-header {
  padding: 10px 20px;
}
/* Title, protocol, channel and model count share one line; the channel name is
   the key identifier, so it carries the weight. */
.m26-modal-heading {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.m26-modal-channel {
  font-weight: 700;
  color: var(--m26-text);
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.m26-modal-count {
  font-size: 12px;
  color: var(--m26-text-muted);
  white-space: nowrap;
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
  overflow: visible;
  position: relative;
  z-index: 2;
}
.m26-model-batch-actions {
  display: flex;
  align-items: center;
  gap: 7px;
}
.m26-model-batch-count {
  padding-right: 3px;
  color: var(--m26-text-muted);
  font-size: 11px;
  font-weight: 500;
  white-space: nowrap;
}
.m26-model-batch-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  padding: 0;
  border: 1px solid var(--m26-border-bright);
  border-radius: 8px;
  background: #fff;
  color: #334155;
  cursor: pointer;
  user-select: none;
  transition: all 0.15s ease;
}
.m26-model-batch-btn:hover:not(:disabled) {
  border-color: rgba(79, 70, 229, 0.35);
  background: rgba(79, 70, 229, 0.05);
  color: var(--m26-accent);
}
.m26-model-batch-btn.injection {
  color: #059669;
  border-color: rgba(5, 150, 105, 0.22);
  background: rgba(5, 150, 105, 0.05);
}
.m26-model-batch-btn.injection:hover:not(:disabled) {
  border-color: rgba(5, 150, 105, 0.38);
  background: rgba(5, 150, 105, 0.1);
  color: #047857;
}
.m26-model-batch-btn:focus-visible {
  outline: 2px solid rgba(79, 70, 229, 0.55);
  outline-offset: 2px;
}
.m26-model-batch-btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.m26-model-batch-btn.probe.is-loading {
  opacity: 1;
  color: #4f46e5;
  border-color: rgba(79, 70, 229, 0.42);
  background: rgba(79, 70, 229, 0.1);
}
.m26-model-batch-btn.probe.is-loading .m26-spin-dot {
  width: 16px;
  height: 16px;
  border-width: 2px;
}
.m26-action-btn.glass-subtle {
  background: rgba(16, 185, 129, 0.08);
  color: #059669;
  border: 1px solid rgba(16, 185, 129, 0.25);
  box-shadow: none;
}
.m26-action-btn.glass-subtle:hover:not(:disabled) {
  background: rgba(16, 185, 129, 0.15);
  border-color: rgba(16, 185, 129, 0.4);
  color: #047857;
}
.m26-action-btn.glass-subtle:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.m26-select-all-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 500;
  color: #475569;
  cursor: pointer;
  user-select: none;
}
.m26-select-all-label:hover {
  color: #0f172a;
}
.m26-select-all-label input[type="checkbox"],
.m26-model-checkbox input[type="checkbox"] {
  width: 15px;
  height: 15px;
  border-radius: 4px;
  cursor: pointer;
  accent-color: #6366f1;
}
.m26-model-checkbox {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  user-select: none;
  flex-shrink: 0;
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
  cursor: pointer;
  transition: all 0.15s ease;
}
.m26-model-item-card:hover {
  background: #f8fafc;
  border-color: rgba(79, 70, 229, 0.25);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.04);
}
.m26-model-item-card.is-selected {
  background: rgba(99, 102, 241, 0.04);
  border-color: rgba(99, 102, 241, 0.35);
}
.m26-model-item-card.is-selected:hover {
  background: rgba(99, 102, 241, 0.08);
}
.m26-primary-slot-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 26px;
  width: 28px;
  box-sizing: border-box;
  padding: 0;
  border-radius: 7px;
  font-weight: 600;
  background: rgba(99, 102, 241, 0.1);
  color: #4f46e5;
  border: 1px solid rgba(99, 102, 241, 0.28);
  white-space: nowrap;
  user-select: none;
  flex-shrink: 0;
}
.m26-btn-micro.set-primary {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 28px;
  box-sizing: border-box;
  padding: 0;
  color: #6366f1;
  background: rgba(99, 102, 241, 0.08);
  border-color: rgba(99, 102, 241, 0.22);
  font-weight: 500;
  flex-shrink: 0;
}
.m26-btn-micro.set-primary:hover {
  color: #4f46e5;
  background: rgba(99, 102, 241, 0.16);
  border-color: rgba(99, 102, 241, 0.4);
}
.m26-btn-micro.probe {
  width: 30px;
  height: 28px;
  box-sizing: border-box;
  padding: 0;
  justify-content: center;
  flex-shrink: 0;
}
.m26-btn-micro.probe.is-loading,
.m26-btn-micro.probe.is-loading:hover {
  color: #4f46e5;
  background: rgba(79, 70, 229, 0.1);
  border-color: rgba(79, 70, 229, 0.42);
  opacity: 1;
  cursor: wait;
}
.m26-model-item-actions .m26-btn-micro.probe.is-loading .m26-spin-dot {
  width: 16px;
  height: 16px;
  border-width: 2px;
}
.m26-btn-micro.inject {
  width: 30px;
  height: 28px;
  box-sizing: border-box;
  padding: 0;
  justify-content: center;
  flex-shrink: 0;
  color: #059669;
  background: rgba(5, 150, 105, 0.08);
  border-color: rgba(5, 150, 105, 0.22);
  font-weight: 500;
}
.m26-btn-micro.inject:hover:not(:disabled) {
  color: #047857;
  background: rgba(5, 150, 105, 0.16);
  border-color: rgba(5, 150, 105, 0.38);
}
.m26-model-item-info {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
  flex: 1;
  user-select: text !important;
  -webkit-user-select: text !important;
}
.m26-model-checked-at {
  color: var(--m26-text-muted);
  font-size: 10px;
  line-height: 1.2;
}
.m26-model-idx {
  font-size: 11px;
  font-family: ui-monospace, monospace;
  color: var(--m26-text-muted);
  font-weight: 600;
  user-select: text !important;
  -webkit-user-select: text !important;
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
  user-select: text !important;
  -webkit-user-select: text !important;
  cursor: text;
}
.m26-model-ch-url {
  font-size: 11px;
  color: var(--m26-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 380px;
  user-select: text !important;
  -webkit-user-select: text !important;
  cursor: text;
}
.m26-model-item-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}
.m26-model-item-actions .m26-btn-micro:focus-visible {
  outline: 2px solid rgba(79, 70, 229, 0.55);
  outline-offset: 2px;
}
.m26-model-lat-pill {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-width: 66px;
  height: 26px;
  box-sizing: border-box;
  padding: 0 8px;
  border-radius: 99px;
  font-size: 11px;
  font-weight: 600;
  font-family: ui-monospace, monospace;
  background: rgba(100, 116, 139, 0.08);
  color: #64748b;
  border: 1px solid rgba(100, 116, 139, 0.15);
  transition: all 0.15s ease;
  flex-shrink: 0;
  white-space: nowrap;
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

.m26-model-inject-pill {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  min-width: 76px;
  height: 26px;
  box-sizing: border-box;
  padding: 0 8px;
  border-radius: 99px;
  font-size: 11px;
  font-weight: 600;
  font-family: ui-monospace, monospace;
  background: rgba(100, 116, 139, 0.06);
  color: #94a3b8;
  border: 1px solid rgba(100, 116, 139, 0.14);
  transition: all 0.15s ease;
  flex-shrink: 0;
  cursor: pointer;
  user-select: none;
  white-space: nowrap;
}
.m26-model-inject-pill:hover {
  filter: brightness(0.95);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.06);
}
.m26-model-inject-pill.clean {
  background: rgba(5, 150, 105, 0.1);
  color: #059669;
  border-color: rgba(5, 150, 105, 0.25);
}
.m26-model-inject-pill.suspected {
  background: rgba(217, 119, 6, 0.12);
  color: #d97706;
  border-color: rgba(217, 119, 6, 0.25);
}
.m26-model-inject-pill.detected {
  background: rgba(220, 38, 38, 0.1);
  color: #dc2626;
  border-color: rgba(220, 38, 38, 0.25);
}
.m26-model-inject-pill.checking {
  background: rgba(79, 70, 229, 0.1);
  color: var(--m26-accent);
  border-color: rgba(79, 70, 229, 0.25);
}
.m26-model-inject-pill.error {
  background: rgba(100, 116, 139, 0.08);
  color: #94a3b8;
  border-color: rgba(100, 116, 139, 0.18);
}
.m26-model-inject-pill.idle {
  background: rgba(100, 116, 139, 0.06);
  color: #94a3b8;
  border-color: rgba(100, 116, 139, 0.12);
}
.m26-model-inject-pill .inject-shield {
  flex-shrink: 0;
}

/* ==========================================================================
   Error Banner & Diagnostic Modal Sheet
   ========================================================================== */
.error-strip-icon {
  flex-shrink: 0;
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
  color: var(--m26-rose);
  margin-left: 2px;
  flex-shrink: 0;
}

/* ==========================================================================
   macOS 26 White: Components & Contrast Utilities
   ========================================================================== */
.text-main {
  color: var(--m26-text) !important;
}

/* Gateway self test card */

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
   Card Selection Styles
   ========================================================================== */
/* Card Selection state */
.m26-table tr.is-selected {
  background-color: rgba(79, 70, 229, 0.06);
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
