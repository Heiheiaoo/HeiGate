<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import type { Channel } from '@/components/channels/types'
import hermesLogoUrl from '@/assets/icons/hermes.png'
import GatewayAccessSettings from '@/components/settings/GatewayAccessSettings.vue'
import RoutingSettings from '@/components/settings/RoutingSettings.vue'
import type { GlobalRouteStrategy } from '@/views/GatewayView.vue'
import { getClientConfigs, updateClientConfig } from '@/services/gatewayApi'
import {
  getCanonicalGroupKey,
  selectCanonicalDisplayName,
} from '@/services/modelNaming'
import {
  type ClientPlatformId,
  type ClientMode,
  type ClientPlatformItem,
  type PlatformSaveEvent,
  type ClientGatewayStrategy,
  filterChannelsForPlatform,
} from './types'

const props = withDefaults(
  defineProps<{
    channels: Channel[]
    gatewayBaseUrl?: string
    gatewayKey?: string
    routingStrategy?: GlobalRouteStrategy
    customModelMappings?: Record<string, string>
  }>(),
  {
    gatewayBaseUrl: 'http://127.0.0.1:28888/v1',
    gatewayKey: '',
    routingStrategy: 'smart_quality',
    customModelMappings: () => ({}),
  }
)

const emit = defineEmits<{
  save: [event: PlatformSaveEvent]
  testConnection: [platformId: ClientPlatformId, channelName: string, model: string]
  'update-routing-strategy': [strategy: GlobalRouteStrategy]
}>()

const activePlatformId = ref<ClientPlatformId>('hermes')
const showDiff = ref(false)
const testState = ref<{ [K in ClientPlatformId]?: 'testing' | 'success' | 'failed' | null }>({})
const testMessage = ref<{ [K in ClientPlatformId]?: string }>({})
const copiedKey = ref(false)
const copiedPath = ref(false)
const copiedEnv = ref(false)
const finderTip = ref('')
const showGenericApiKey = ref(false)

const maskedGenericApiKey = computed(() => {
  const k = props.gatewayKey || ''
  if (!k) return '••••••••'
  if (k.length <= 16) return '••••••••'
  return `${k.slice(0, 8)}••••••••${k.slice(-6)}`
})

// 平台展示顺序：Hermes -> Codex -> Claude Desktop -> Claude Code -> 通用
const PLATFORM_ORDER: ClientPlatformId[] = ['hermes', 'codex', 'claude-desktop', 'claude-code', 'generic']

// 初始默认平台列表及各自个性化配置
const platforms = ref<Record<ClientPlatformId, ClientPlatformItem>>({
  'hermes': {
    id: 'hermes',
    name: 'Hermes',
    categoryLabel: '全能自主智能体',
    description: '自主智能体系统，深度融合长上下文思考与多工具调用，依托 HeiGate 网关专享调度。',
    configPath: '~/.hermes/config.yaml',
    supportedModes: ['gateway'],
    activeMode: 'gateway',
    direct: {
      channelName: '',
      endpoint: '',
      apiKey: '',
      model: '',
    },
    gateway: {
      strategy: 'smart_quality',
      allowedChannels: [],
      model: 'claude-3-7-sonnet-latest',
      fallbackChannel: '',
      allowedModels: [],
    },
    status: 'stopped',
    fileStatus: 'synced',
    lastBackupTime: '刚刚',
  },
  'codex': {
    id: 'codex',
    name: 'Codex',
    categoryLabel: 'OpenAI 规范编程工具',
    description: 'OpenAI 生态代码辅助工具与终端智能体，支持官方协议直连与网关容灾调度。',
    configPath: '~/.codex/config.json',
    supportedModes: ['direct', 'gateway'],
    activeMode: 'direct',
    direct: {
      channelName: '',
      endpoint: '',
      apiKey: '',
      model: 'gpt-4o',
    },
    gateway: {
      strategy: 'smart_quality',
      allowedChannels: [],
      model: 'gpt-4o',
      fallbackChannel: '',
      allowedModels: [],
    },
    status: 'stopped',
    fileStatus: 'synced',
    lastBackupTime: '刚刚',
  },
  'claude-desktop': {
    id: 'claude-desktop',
    name: 'Claude Desktop',
    categoryLabel: 'macOS 原生桌面应用',
    description: 'Anthropic 官方桌面应用，支持多窗口交互与原生 MCP (Model Context Protocol) 本地插件服务。',
    configPath: '~/Library/Application Support/Claude/claude_desktop_config.json',
    supportedModes: ['direct', 'gateway'],
    activeMode: 'direct',
    direct: {
      channelName: '',
      endpoint: '',
      apiKey: '',
      model: 'claude-3-7-sonnet-latest',
    },
    gateway: {
      strategy: 'smart_quality',
      allowedChannels: [],
      model: 'claude-3-7-sonnet-latest',
      fallbackChannel: '',
      allowedModels: [],
    },
    status: 'stopped',
    fileStatus: 'synced',
    lastBackupTime: '刚刚',
  },
  'claude-code': {
    id: 'claude-code',
    name: 'Claude Code',
    categoryLabel: 'Anthropic 终端 CLI',
    description: 'Anthropic 官方终端智能体，深度集成 Bash 执行、全局代码重构与双模型架构。',
    configPath: '~/.claude.json',
    supportedModes: ['direct', 'gateway'],
    activeMode: 'direct',
    direct: {
      channelName: '',
      endpoint: '',
      apiKey: '',
      model: 'claude-3-7-sonnet-latest',
      smallModel: 'claude-3-5-haiku-latest',
    },
    gateway: {
      strategy: 'latency',
      allowedChannels: [],
      model: 'claude-3-7-sonnet-latest',
      fallbackChannel: '',
      allowedModels: [],
    },
    status: 'stopped',
    fileStatus: 'synced',
    lastBackupTime: '刚刚',
  },
  'generic': {
    id: 'generic',
    name: '通用',
    categoryLabel: '标准 API 网关',
    description: 'Cursor / Windsurf / Cline / Continue / Python SDK 等任何客户端的标准 OpenAI / Anthropic 协议接入。',
    configPath: '标准 HTTP Bearer 鉴权',
    supportedModes: ['gateway'],
    activeMode: 'gateway',
    direct: {
      channelName: '',
      endpoint: '',
      apiKey: '',
      model: '',
    },
    gateway: {
      strategy: 'smart_quality',
      allowedChannels: [],
      model: 'gpt-4o',
      fallbackChannel: '',
      allowedModels: [],
    },
    status: 'running',
    fileStatus: 'synced',
    lastBackupTime: '实时在线',
  },
})

const orderedPlatforms = computed(() => PLATFORM_ORDER.map((id) => platforms.value[id]))

// 本地草稿恢复
const STORAGE_KEY = 'heigate_client_platforms_draft'
if (typeof localStorage !== 'undefined') {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      const parsed = JSON.parse(raw)
      for (const key of Object.keys(parsed) as ClientPlatformId[]) {
        if (platforms.value[key]) {
          platforms.value[key].activeMode = parsed[key].activeMode || platforms.value[key].activeMode
          platforms.value[key].direct = { ...platforms.value[key].direct, ...parsed[key].direct }
          platforms.value[key].gateway = { ...platforms.value[key].gateway, ...parsed[key].gateway }
        }
      }
    }
  } catch {
    // 忽略解析错误
  }
}

// 自动持久化草稿
watch(
  platforms,
  (val) => {
    if (typeof localStorage !== 'undefined') {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(val))
      } catch {
        // 忽略存储错误
      }
    }
  },
  { deep: true }
)

onMounted(async () => {
  try {
    const remoteConfigs = await getClientConfigs()
    if (remoteConfigs && typeof remoteConfigs === 'object') {
      for (const [id, cfg] of Object.entries(remoteConfigs)) {
        const platId = id.replace('_', '-') as ClientPlatformId
        if (platforms.value[platId]) {
          if (cfg.strategy) platforms.value[platId].gateway.strategy = cfg.strategy as any
          if (cfg.allowed_channels) platforms.value[platId].gateway.allowedChannels = cfg.allowed_channels
          if (cfg.model) platforms.value[platId].gateway.model = cfg.model
          if (cfg.small_model) platforms.value[platId].direct.smallModel = cfg.small_model
          if (cfg.allowed_models) platforms.value[platId].gateway.allowedModels = cfg.allowed_models
          if (platId === 'hermes') {
            isHermesAutoEnabled.value = Boolean(cfg.auto_enabled ?? (cfg.model === 'Auto' || true))
            if (cfg.auto_strategy) {
              hermesStrategy.value = cfg.auto_strategy as any
            } else if (cfg.strategy) {
              hermesStrategy.value = cfg.strategy as any
            }
            if (cfg.allowed_channels && cfg.allowed_channels.length > 0) {
              hermesSelectedChannels.value = [...cfg.allowed_channels]
            }
            if (cfg.allowed_models && cfg.allowed_models.length > 0) {
              platforms.value['hermes'].gateway.allowedModels = [...cfg.allowed_models]
            }
          }
        }
      }
    }
  } catch {
    // 忽略远端加载错误
  }
})

const currentPlatform = computed(() => platforms.value[activePlatformId.value])

// 当前平台兼容的供应商列表
const compatibleChannels = computed(() => {
  return filterChannelsForPlatform(activePlatformId.value, props.channels)
})

// 当切换平台或供应商列表更新时，如果当前直连供应商为空，自动选取第一个兼容渠道
watch(
  [activePlatformId, compatibleChannels],
  ([platId, compChans]) => {
    const plat = platforms.value[platId]
    if (plat && plat.supportedModes.includes('direct') && !plat.direct.channelName && compChans.length > 0) {
      selectDirectChannel(compChans[0].name)
    }
  },
  { immediate: true }
)

function selectDirectChannel(channelName: string) {
  const plat = currentPlatform.value
  plat.direct.channelName = channelName
  const found = props.channels.find((c) => c.name === channelName)
  if (found) {
    plat.direct.endpoint = found.url || ''
    plat.direct.apiKey = found.apiKey || ''
    const availableModels = found.modelNames && found.modelNames.length > 0 ? found.modelNames : found.primaryModel ? [found.primaryModel] : []
    if (availableModels.length > 0 && !availableModels.includes(plat.direct.model)) {
      plat.direct.model = availableModels[0]
    }
  }
}

const currentChannelModels = computed(() => {
  const found = props.channels.find((c) => c.name === currentPlatform.value.direct.channelName)
  if (!found) return []
  const list = [...(found.modelNames || [])]
  if (found.primaryModel && !list.includes(found.primaryModel)) {
    list.unshift(found.primaryModel)
  }
  return list
})

function switchMode(mode: ClientMode) {
  if (currentPlatform.value.supportedModes.includes(mode)) {
    currentPlatform.value.activeMode = mode
  }
}

function handleSave(launchAfterSave = false) {
  const plat = currentPlatform.value
  if (plat.id === 'hermes') {
    void applyHermesConfig(launchAfterSave ? 'restart' : 'save_only')
    return
  }
  plat.fileStatus = 'synced'
  emit('save', {
    platformId: plat.id,
    mode: plat.activeMode,
    direct: { ...plat.direct },
    gateway: { ...plat.gateway },
    launchAfterSave,
  })
}

async function copyText(text: string, isKey = false) {
  try {
    await navigator.clipboard.writeText(text)
    if (isKey) {
      copiedKey.value = true
      setTimeout(() => (copiedKey.value = false), 2000)
    } else {
      copiedPath.value = true
      setTimeout(() => (copiedPath.value = false), 2000)
    }
  } catch {
    // 忽略剪贴板错误
  }
}

const gatewayOrigin = computed(() => {
  return props.gatewayBaseUrl.replace(/\/v1\/?$/, '').replace(/\/+$/, '')
})

function getPlatformBaseUrl(id: ClientPlatformId): string {
  const origin = gatewayOrigin.value
  switch (id) {
    case 'hermes':
      return `${origin}/hermes/v1`
    case 'codex':
      return `${origin}/codex/v1`
    case 'claude-desktop':
      return `${origin}/claude/v1`
    case 'claude-code':
      return `${origin}/claude_code`
    case 'generic':
    default:
      return `${origin}/v1`
  }
}

async function copyEnvScript(platformId: ClientPlatformId) {
  let script = ''
  const key = props.gatewayKey || 'hg_gateway_secret_key'
  if (platformId === 'hermes') {
    const model = isHermesAutoEnabled.value ? 'Auto' : (platforms.value['hermes'].gateway.model || 'Auto')
    script = `export OPENAI_BASE_URL="${getPlatformBaseUrl('hermes')}"\nexport OPENAI_API_KEY="${key}"\nexport HERMES_MODEL="${model}"`
  } else if (platformId === 'codex') {
    script = `export OPENAI_BASE_URL="${getPlatformBaseUrl('codex')}"\nexport OPENAI_API_KEY="${key}"`
  } else if (platformId === 'claude-code') {
    script = `export ANTHROPIC_BASE_URL="${getPlatformBaseUrl('claude-code')}"\nexport ANTHROPIC_API_KEY="${key}"`
  } else if (platformId === 'generic') {
    script = `export OPENAI_BASE_URL="${getPlatformBaseUrl('generic')}"\nexport OPENAI_API_KEY="${key}"`
  }
  if (script) {
    try {
      await navigator.clipboard.writeText(script)
      copiedEnv.value = true
      setTimeout(() => (copiedEnv.value = false), 2500)
    } catch {
      // 忽略
    }
  }
}

// Hermes 专享：供应商多选、模型多选与调度状态
const hermesSelectedChannels = ref<string[]>([])
const isApplyingLocalHermes = ref(false)
const hermesActionMessage = ref<{ type: 'success' | 'error'; text: string } | null>(null)
const isHermesAutoEnabled = ref(true)
const showHermesAutoHelp = ref(false)
const hermesStrategy = ref<ClientGatewayStrategy>('smart_quality')
const isHermesStrategyDropdownOpen = ref(false)

const hermesStrategyOptions = [
  { value: 'smart_quality' as const, label: '智能综合质量优先' },
  { value: 'latency' as const, label: '最低首字延迟' },
  { value: 'round_robin' as const, label: '轮询负载均衡' },
]

const currentHermesStrategyLabel = computed(() => {
  const opt = hermesStrategyOptions.find((o) => o.value === hermesStrategy.value)
  return opt ? opt.label : '智能综合质量优先'
})

function onToggleHermesAuto() {
  if (isHermesAutoEnabled.value) {
    if (effectiveHermesAllowedModels.value.length === 0) {
      platforms.value['hermes'].gateway.allowedModels = [...allHermesAvailableModelNames.value]
    }
  } else {
    isHermesStrategyDropdownOpen.value = false
  }
}

function setHermesStrategy(s: ClientGatewayStrategy) {
  platforms.value['hermes'].gateway.strategy = s
  hermesStrategy.value = s
}

function selectHermesStrategy(s: ClientGatewayStrategy) {
  setHermesStrategy(s)
  isHermesStrategyDropdownOpen.value = false
}

// 仅展示已启用的供应商（用户要求：供应商只要启用的）
const hermesAvailableChannels = computed(() => {
  return props.channels.filter((c) => c.enabled)
})

// 自动选定 Hermes 初始供应商（仅在首次加载时默认选中已有配置或全部可用供应商）
let hasInitializedHermesChannels = false
watch(
  hermesAvailableChannels,
  (chans) => {
    if (!hasInitializedHermesChannels && chans.length > 0) {
      hasInitializedHermesChannels = true
      const allowed = platforms.value['hermes']?.gateway.allowedChannels
      if (allowed && allowed.length > 0) {
        const matched = allowed.filter((name) => chans.some((c) => c.name === name))
        if (matched.length > 0) {
          hermesSelectedChannels.value = [...matched]
          return
        }
      }
      if (hermesSelectedChannels.value.length === 0) {
        const allNames = chans.map((c) => c.name)
        hermesSelectedChannels.value = [...allNames]
        platforms.value['hermes'].gateway.allowedChannels = [...allNames]
      }
    }
  },
  { immediate: true }
)

// 供应商多选操作
function isHermesChannelSelected(channelName: string): boolean {
  return hermesSelectedChannels.value.includes(channelName)
}

function toggleHermesChannel(channelName: string) {
  const current = [...hermesSelectedChannels.value]
  const idx = current.indexOf(channelName)
  if (idx >= 0) {
    current.splice(idx, 1)
  } else {
    current.push(channelName)
  }
  hermesSelectedChannels.value = current
  platforms.value['hermes'].gateway.allowedChannels = [...current]
}

function selectAllHermesChannels() {
  const allNames = hermesAvailableChannels.value.map((c) => c.name)
  hermesSelectedChannels.value = [...allNames]
  platforms.value['hermes'].gateway.allowedChannels = [...allNames]
}

function clearHermesChannels() {
  hermesSelectedChannels.value = []
  platforms.value['hermes'].gateway.allowedChannels = []
}

const isAllHermesChannelsSelected = computed(() => {
  const total = hermesAvailableChannels.value.length
  if (total === 0) return false
  const availableNames = new Set(hermesAvailableChannels.value.map((c) => c.name))
  const selectedCount = hermesSelectedChannels.value.filter((n) => availableNames.has(n)).length
  return selectedCount === total
})

function toggleAllHermesChannels() {
  if (isAllHermesChannelsSelected.value) {
    clearHermesChannels()
  } else {
    selectAllHermesChannels()
  }
}

function getChannelModelCount(ch: Channel): number {
  if (ch.modelNames && ch.modelNames.length > 0) return ch.modelNames.length
  return ch.primaryModel ? 1 : 0
}

// 图二模型分类规则 (Model Family Categorization Rules)
interface ModelFamily {
  key: string
  label: string
}

function getModelFamily(name: string): ModelFamily {
  const n = (name || '').toLowerCase()
  if (n === 'auto') {
    return { key: 'other', label: '智能路由' }
  }
  if (n.includes('deepseek')) {
    return { key: 'deepseek', label: 'DeepSeek' }
  }
  if (n.includes('claude')) {
    return { key: 'claude', label: 'Claude' }
  }
  if (n.includes('gpt') || n.startsWith('o1') || n.startsWith('o3') || n.includes('chatgpt')) {
    return { key: 'openai', label: 'OpenAI' }
  }
  if (n.includes('qwen')) {
    return { key: 'qwen', label: 'Qwen' }
  }
  if (n.includes('glm')) {
    return { key: 'glm', label: '智谱 GLM' }
  }
  if (n.includes('kimi') || n.includes('moonshot')) {
    return { key: 'kimi', label: 'Kimi' }
  }
  if (n.includes('minimax')) {
    return { key: 'minimax', label: 'MiniMax' }
  }
  if (n.includes('mimo')) {
    return { key: 'mimo', label: 'MiMO' }
  }
  if (n.includes('doubao') || n.includes('skylark')) {
    return { key: 'doubao', label: '豆包' }
  }
  if (n.includes('gemini')) {
    return { key: 'gemini', label: 'Gemini' }
  }
  if (n.includes('llama')) {
    return { key: 'llama', label: 'Llama' }
  }
  return { key: 'other', label: '其他' }
}

const MODEL_FAMILY_ORDER = ['deepseek', 'qwen', 'glm', 'kimi', 'minimax', 'mimo', 'doubao', 'claude', 'openai', 'gemini', 'llama', 'other'] as const
const MODEL_FAMILY_LABELS: Record<string, string> = {
  deepseek: 'DeepSeek',
  claude: 'Claude',
  openai: 'OpenAI',
  qwen: 'Qwen',
  glm: '智谱 GLM',
  kimi: 'Kimi',
  minimax: 'MiniMax',
  mimo: 'MiMO',
  doubao: '豆包',
  gemini: 'Gemini',
  llama: 'Llama',
  other: '其他',
}

// 汇聚已选供应商的去重模型，按归并后的来 (Canonical / Merged Models)
interface MergedModelItem {
  name: string
  canonicalKey: string
  rawNames: string[]
  providers: string[]
  aliases: string[]
  familyKey: string
  familyLabel: string
}

const selectedHermesModelFamily = ref<string>('all')

const hermesMergedModels = computed<MergedModelItem[]>(() => {
  const groupMap = new Map<string, {
    canonicalKey: string
    rawNames: Set<string>
    providers: Set<string>
  }>()

  const mappings = props.customModelMappings || {}

  for (const ch of hermesAvailableChannels.value) {
    if (!hermesSelectedChannels.value.includes(ch.name)) continue
    const list = (ch.modelNames && ch.modelNames.length > 0)
      ? ch.modelNames
      : (ch.primaryModel ? [ch.primaryModel] : [])
    for (const raw of list) {
      const trimmed = (raw || '').trim()
      if (!trimmed) continue
      const groupKey = getCanonicalGroupKey(trimmed, mappings) || trimmed.toLowerCase()
      const current = groupMap.get(groupKey) || {
        canonicalKey: groupKey,
        rawNames: new Set<string>(),
        providers: new Set<string>(),
      }
      current.rawNames.add(trimmed)
      current.providers.add(ch.name)
      groupMap.set(groupKey, current)
    }
  }

  const result: MergedModelItem[] = []
  groupMap.forEach((data, groupKey) => {
    const allRaw = Array.from(data.rawNames)
    const displayName = selectCanonicalDisplayName(allRaw, mappings) || allRaw[0]
    const aliases = allRaw.filter((r) => r.toLowerCase() !== displayName.toLowerCase())
    const family = getModelFamily(displayName)
    result.push({
      name: displayName,
      canonicalKey: groupKey,
      rawNames: allRaw,
      providers: Array.from(data.providers),
      aliases,
      familyKey: family.key,
      familyLabel: family.label,
    })
  })

  return result.sort((a, b) => a.name.localeCompare(b.name))
})

// 图二模型分类 Tabs
const hermesModelFamilyTabs = computed(() => {
  const counts: Record<string, number> = {}
  hermesMergedModels.value.forEach((m) => {
    const fam = m.familyKey
    counts[fam] = (counts[fam] || 0) + 1
  })
  const tabs = [{ key: 'all', label: '全部', count: hermesMergedModels.value.length }]
  MODEL_FAMILY_ORDER.forEach((k) => {
    if (counts[k]) {
      tabs.push({ key: k, label: MODEL_FAMILY_LABELS[k] || k, count: counts[k] })
    }
  })
  return tabs
})

const allHermesAvailableModelNames = computed<string[]>(() => {
  return hermesMergedModels.value.map((m) => m.name)
})

// 有效已选模型：严格限定在当前已选供应商的归并模型池中
const effectiveHermesAllowedModels = computed<string[]>({
  get() {
    const list = platforms.value['hermes']?.gateway.allowedModels || []
    return list.filter((m) => allHermesAvailableModelNames.value.includes(m))
  },
  set(newVal) {
    if (platforms.value['hermes']) {
      platforms.value['hermes'].gateway.allowedModels = [...newVal]
    }
  },
})



// 监听当前已选供应商的模型变化，自动校准与同步
watch(
  allHermesAvailableModelNames,
  (available) => {
    if (!platforms.value['hermes']) return
    const current = platforms.value['hermes'].gateway.allowedModels || []
    const valid = current.filter((m) => available.includes(m))
    // 如果之前保存的模型无任何交集（例如切换了完全不同的供应商），或者未初始化过，则默认全选当前供应商的归并模型
    if (valid.length === 0 && available.length > 0) {
      platforms.value['hermes'].gateway.allowedModels = [...available]
      platforms.value['hermes'].gateway.model = available[0]
    } else {
      platforms.value['hermes'].gateway.allowedModels = valid
      if (valid.length > 0 && (!platforms.value['hermes'].gateway.model || !valid.includes(platforms.value['hermes'].gateway.model))) {
        platforms.value['hermes'].gateway.model = valid[0]
      }
    }
  },
  { immediate: true }
)

const filteredHermesModels = computed(() => {
  let list = hermesMergedModels.value
  if (selectedHermesModelFamily.value !== 'all') {
    list = list.filter((m) => m.familyKey === selectedHermesModelFamily.value)
  }
  return list
})

const currentFilterSelectedCount = computed(() => {
  return filteredHermesModels.value.filter((m) => effectiveHermesAllowedModels.value.includes(m.name)).length
})

watch(hermesModelFamilyTabs, () => {
  if (selectedHermesModelFamily.value !== 'all' && !hermesModelFamilyTabs.value.some((t) => t.key === selectedHermesModelFamily.value)) {
    selectedHermesModelFamily.value = 'all'
  }
})

const hermesTabsScrollRef = ref<HTMLElement | null>(null)

function onHermesTabsWheel(e: WheelEvent) {
  if (!hermesTabsScrollRef.value) return
  // If user is swiping horizontally with trackpad, let native browser scroll naturally
  if (Math.abs(e.deltaX) > 0) {
    return
  }
  // If user scrolls with standard mouse vertical wheel, translate to horizontal scroll
  if (e.deltaY !== 0) {
    e.preventDefault()
    hermesTabsScrollRef.value.scrollLeft += e.deltaY
  }
}



// 模型多选操作
function isHermesModelSelected(modelName: string): boolean {
  return effectiveHermesAllowedModels.value.includes(modelName)
}

function toggleHermesModel(modelName: string) {
  const current = [...effectiveHermesAllowedModels.value]
  const idx = current.indexOf(modelName)
  if (idx >= 0) {
    current.splice(idx, 1)
  } else {
    current.push(modelName)
  }
  effectiveHermesAllowedModels.value = current
  if (current.length > 0 && (!platforms.value['hermes'].gateway.model || !current.includes(platforms.value['hermes'].gateway.model))) {
    platforms.value['hermes'].gateway.model = current[0]
  }
}

function selectAllHermesModels() {
  if (selectedHermesModelFamily.value === 'all') {
    platforms.value['hermes'].gateway.allowedModels = [...allHermesAvailableModelNames.value]
  } else {
    const familyModels = hermesMergedModels.value
      .filter((m) => m.familyKey === selectedHermesModelFamily.value)
      .map((m) => m.name)
    const set = new Set([...effectiveHermesAllowedModels.value, ...familyModels])
    platforms.value['hermes'].gateway.allowedModels = Array.from(set)
  }
  if (platforms.value['hermes'].gateway.allowedModels.length > 0) {
    platforms.value['hermes'].gateway.model = platforms.value['hermes'].gateway.allowedModels[0]
  }
}

function clearHermesModels() {
  if (selectedHermesModelFamily.value === 'all') {
    platforms.value['hermes'].gateway.allowedModels = []
  } else {
    const familyModels = new Set(
      hermesMergedModels.value
        .filter((m) => m.familyKey === selectedHermesModelFamily.value)
        .map((m) => m.name)
    )
    platforms.value['hermes'].gateway.allowedModels = effectiveHermesAllowedModels.value.filter((m) => !familyModels.has(m))
  }
}

const isAllHermesModelsSelected = computed(() => {
  if (selectedHermesModelFamily.value === 'all') {
    return (
      allHermesAvailableModelNames.value.length > 0 &&
      effectiveHermesAllowedModels.value.length === allHermesAvailableModelNames.value.length
    )
  }
  const familyModels = hermesMergedModels.value
    .filter((m) => m.familyKey === selectedHermesModelFamily.value)
    .map((m) => m.name)
  return (
    familyModels.length > 0 &&
    familyModels.every((m) => effectiveHermesAllowedModels.value.includes(m))
  )
})



function toggleAllHermesModels() {
  if (isAllHermesModelsSelected.value) {
    clearHermesModels()
  } else {
    selectAllHermesModels()
  }
}


async function applyHermesConfig(action: 'restart' | 'launch_terminal' | 'save_only') {
  isApplyingLocalHermes.value = true
  hermesActionMessage.value = null
  try {
    const plat = platforms.value['hermes']
    const allowedChannels = hermesSelectedChannels.value.length > 0
      ? hermesSelectedChannels.value
      : (plat.gateway.allowedChannels || [])
    const allowedModels = effectiveHermesAllowedModels.value.length > 0
      ? effectiveHermesAllowedModels.value
      : allHermesAvailableModelNames.value
    const effectiveModel = isHermesAutoEnabled.value
      ? 'Auto'
      : (allowedModels[0] || plat.gateway.model || 'Auto')
    const payload = {
      strategy: hermesStrategy.value,
      allowed_channels: allowedChannels,
      model: effectiveModel,
      small_model: plat.direct.smallModel,
      allowed_models: allowedModels,
      auto_enabled: isHermesAutoEnabled.value,
      auto_strategy: hermesStrategy.value,
      auto_models: allowedModels,
      apply_local: action !== 'save_only',
      restart: action === 'restart',
      launch_terminal: action === 'launch_terminal',
    }
    const res = await updateClientConfig('hermes', payload)
    if (res && res.ok) {
      hermesActionMessage.value = {
        type: 'success',
        text: res.message || (action === 'restart' ? '✅ 已更新 ~/.hermes/config.yaml 并成功重启 Hermes 网关！' : action === 'launch_terminal' ? '💻 已更新配置并在终端启动 Hermes！' : '💾 Hermes 配置已成功保存！'),
      }
    } else {
      hermesActionMessage.value = {
        type: 'error',
        text: res?.message || '❌ 保存失败，请检查服务连接',
      }
    }
  } catch (err: any) {
    hermesActionMessage.value = {
      type: 'error',
      text: `❌ 操作异常: ${err?.message || err}`,
    }
  } finally {
    isApplyingLocalHermes.value = false
  }
}


function revealInFinder() {
  const folderPath = '~/Library/Application Support/Claude/'
  void copyText(folderPath)
  finderTip.value = '已复制目录路径！在 Finder 中按 Cmd+Shift+G 即可直达'
  setTimeout(() => (finderTip.value = ''), 4000)
}

function quickSelectModel(modelName: string, isDirect = false) {
  if (isDirect) {
    currentPlatform.value.direct.model = modelName
  } else {
    currentPlatform.value.gateway.model = modelName
  }
}

function quickSelectSmallModel(modelName: string) {
  currentPlatform.value.direct.smallModel = modelName
}

function runTestConnection() {
  const plat = currentPlatform.value
  testState.value[plat.id] = 'testing'
  testMessage.value[plat.id] = '正在探测上游响应与延迟...'
  setTimeout(() => {
    if (plat.direct.channelName && plat.direct.endpoint) {
      testState.value[plat.id] = 'success'
      testMessage.value[plat.id] = '连通性验证成功（HTTP 200 OK · 实测延迟 168ms）'
    } else {
      testState.value[plat.id] = 'failed'
      testMessage.value[plat.id] = '连接失败：请先选择有效供应商与端点'
    }
    setTimeout(() => {
      testState.value[plat.id] = null
    }, 4500)
  }, 600)
}

function toggleGatewayChannel(chanName: string) {
  const list = currentPlatform.value.gateway.allowedChannels
  const idx = list.indexOf(chanName)
  if (idx >= 0) {
    list.splice(idx, 1)
  } else {
    list.push(chanName)
  }
}

function getLaunchButtonText(id: ClientPlatformId): string {
  switch (id) {
    case 'hermes':
      return '保存并在终端启动 Hermes'
    case 'codex':
      return '保存并启动 Codex'
    case 'claude-desktop':
      return '保存并启动 Claude'
    case 'claude-code':
      return '保存并在终端打开'
    case 'generic':
      return '保存网关配置'
  }
}

// 针对每个客户端定制的个性化差量 Diff 预览
const diffPreview = computed(() => {
  const plat = currentPlatform.value
  const keyDisplay = props.gatewayKey ? `${props.gatewayKey.slice(0, 8)}****************` : 'hg_********************************'
  if (plat.id === 'hermes') {
    const allowed = (effectiveHermesAllowedModels.value.length > 0)
      ? effectiveHermesAllowedModels.value.join(', ')
      : (allHermesAvailableModelNames.value.join(', ') || 'Auto')
    const effectiveModel = isHermesAutoEnabled.value
      ? 'Auto'
      : (effectiveHermesAllowedModels.value[0] || plat.gateway.model || 'Auto')
    return `# ~/.hermes/config.yaml
model:
  default: ${effectiveModel}
  provider: bend
  base_url: ${getPlatformBaseUrl('hermes')}
  key_env: HERMES_CUSTOM_BEND_API_KEY
providers:
  bend:
    name: 本地网关
    base_url: ${getPlatformBaseUrl('hermes')}
    model: ${effectiveModel}
    discover_models: true
    models: [${allowed}]
# ~/.hermes/.env
HERMES_CUSTOM_BEND_API_KEY=${keyDisplay}`
  }

  if (plat.id === 'claude-desktop') {
    if (plat.activeMode === 'direct') {
      return `// ~/Library/Application Support/Claude/claude_desktop_config.json
{
  "endpoint": "${plat.direct.endpoint || 'https://api.anthropic.com'}",
  "apiKey": "${plat.direct.apiKey ? 'sk-ant-*** (已加密凭证)' : '未配置'}",
  "model": "${plat.direct.model || 'claude-3-7-sonnet-latest'}",
  // 🛡️ 您的 mcpServers 插件服务完整保留:
  "mcpServers": {
    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"] }
  }
}`
    } else {
      return `// ~/Library/Application Support/Claude/claude_desktop_config.json
{
  "endpoint": "${getPlatformBaseUrl('claude-desktop')}",
  "apiKey": "${props.gatewayKey || 'heigate-local-gateway-key'}",
  "model": "${plat.gateway.model || 'claude-3-7-sonnet-latest'}",
  "routing_policy": "${plat.gateway.strategy}",
  // 🛡️ 您的 mcpServers 插件服务完整保留:
  "mcpServers": {
    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"] }
  }
}`
    }
  }

  if (plat.id === 'claude-code') {
    if (plat.activeMode === 'direct') {
      return `// ~/.claude.json
{
  "primaryModel": "${plat.direct.model || 'claude-3-7-sonnet-latest'}",
  "smallFastModel": "${plat.direct.smallModel || 'claude-3-5-haiku-latest'}",
  "endpoint": "${plat.direct.endpoint || 'https://api.anthropic.com'}",
  "apiKey": "${plat.direct.apiKey ? 'sk-ant-***' : '未配置'}"
}`
    } else {
      return `// ~/.claude.json
{
  "primaryModel": "${plat.gateway.model || 'claude-3-7-sonnet-latest'}",
  "smallFastModel": "${plat.direct.smallModel || 'claude-3-5-haiku-latest'}",
  "endpoint": "${getPlatformBaseUrl('claude-code')}",
  "apiKey": "${props.gatewayKey || 'heigate-local-gateway-key'}"
}`
    }
  }

  if (plat.id === 'codex') {
    if (plat.activeMode === 'direct') {
      return `// ~/.codex/config.json
{
  "model": "${plat.direct.model || 'gpt-4o'}",
  "api_endpoint": "${plat.direct.endpoint || 'https://api.openai.com/v1'}",
  "api_key": "${plat.direct.apiKey ? 'sk-***' : '未配置'}"
}`
    } else {
      return `// ~/.codex/config.json
{
  "model": "${plat.gateway.model || 'gpt-4o'}",
  "api_endpoint": "${getPlatformBaseUrl('codex')}",
  "api_key": "${props.gatewayKey || 'heigate-local-gateway-key'}",
  "routing_policy": "${plat.gateway.strategy}"
}`
    }
  }

  return `// ${plat.configPath}\n// HeiGate 差量合并预览`
})
</script>

<template>
  <div class="m26-clients-layout">
    <!-- 顶部：5 联独立客户端切换卡片 (Horizontal Separate Bento Cards) -->
    <nav class="m26-clients-nav-deck" aria-label="客户端平台选择">
      <button
        v-for="plat in orderedPlatforms"
        :key="plat.id"
        class="m26-client-tab-card"
        :class="[plat.id, { active: activePlatformId === plat.id }]"
        :title="plat.name"
        @click="activePlatformId = plat.id"
      >
        <!-- 品牌图标盒 -->
        <div class="m26-tab-logo-box" :class="plat.id">
          <!-- Hermes: Nous Research 官方动漫头像 -->
          <template v-if="plat.id === 'hermes'">
            <img :src="hermesLogoUrl" alt="Hermes" class="m26-deck-hermes-img" />
          </template>

          <!-- Codex: OpenAI 官方螺旋徽标 -->
          <template v-else-if="plat.id === 'codex'">
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <path d="M21.55 10.004a5.416 5.416 0 00-.478-4.501c-1.217-2.09-3.662-3.166-6.05-2.66A5.59 5.59 0 0010.831 1C8.39.995 6.224 2.546 5.473 4.838A5.553 5.553 0 001.76 7.496a5.487 5.487 0 00.691 6.5 5.416 5.416 0 00.477 4.502c1.217 2.09 3.662 3.165 6.05 2.66A5.586 5.586 0 0013.168 23c2.443.006 4.61-1.546 5.361-3.84a5.553 5.553 0 003.715-2.66 5.488 5.488 0 00-.693-6.497v.001zm-8.381 11.558a4.199 4.199 0 01-2.675-.954c.034-.018.093-.05.132-.074l4.44-2.53a.71.71 0 00.364-.623v-6.176l1.877 1.069c.02.01.033.029.036.05v5.115c-.003 2.274-1.87 4.118-4.174 4.123zM4.192 17.78a4.059 4.059 0 01-.498-2.763c.032.02.09.055.131.078l4.44 2.53c.225.13.504.13.73 0l5.42-3.088v2.138a.068.068 0 01-.027.057L9.9 19.288c-1.999 1.136-4.552.46-5.707-1.51h-.001zM3.023 8.216A4.15 4.15 0 015.198 6.41l-.002.151v5.06a.711.711 0 00.364.624l5.42 3.087-1.876 1.07a.067.067 0 01-.063.005l-4.489-2.559c-1.995-1.14-2.679-3.658-1.53-5.63h.001zm15.417 3.54l-5.42-3.088L14.896 7.6a.067.067 0 01.063-.006l4.489 2.557c1.998 1.14 2.683 3.662 1.529 5.633a4.163 4.163 0 01-2.174 1.807V12.38a.71.71 0 00-.363-.623zm1.867-2.773a6.04 6.04 0 00-.132-.078l-4.44-2.53a.731.731 0 00-.729 0l-5.42 3.088V7.325a.068.068 0 01.027-.057L14.1 4.713c2-1.137 4.555-.46 5.707 1.513.487.833.664 1.809.499 2.757h.001zm-11.741 3.81l-1.877-1.068a.065.065 0 01-.036-.051V6.559c.001-2.277 1.873-4.122 4.181-4.12.976 0 1.92.338 2.671.954-.034.018-.092.05-.131.073l-4.44 2.53a.71.71 0 00-.365.623l-.003 6.173v.002zm1.02-2.168L12 9.25l2.414 1.375v2.75L12 14.75l-2.415-1.375v-2.75z" />
            </svg>
          </template>

          <!-- Claude Desktop: 官方 Claude 标志 + 桌面角标 -->
          <template v-else-if="plat.id === 'claude-desktop'">
            <svg viewBox="0 0 24 24" width="22" height="22" fill="#D97757">
              <path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" />
            </svg>
            <span class="m26-glyph-badge desktop">
              <svg viewBox="0 0 24 24" width="8" height="8" fill="none" stroke="currentColor" stroke-width="2.5">
                <rect x="2" y="3" width="20" height="14" rx="2" />
                <line x1="8" y1="21" x2="16" y2="21" />
                <line x1="12" y1="17" x2="12" y2="21" />
              </svg>
            </span>
          </template>

          <!-- Claude Code: 官方 Claude 标志 + 终端角标 -->
          <template v-else-if="plat.id === 'claude-code'">
            <svg viewBox="0 0 24 24" width="22" height="22" fill="#D97757">
              <path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" />
            </svg>
            <span class="m26-glyph-badge terminal">
              <svg viewBox="0 0 24 24" width="8" height="8" fill="none" stroke="currentColor" stroke-width="3">
                <polyline points="4 17 10 11 4 5" />
                <line x1="12" y1="19" x2="20" y2="19" />
              </svg>
            </span>
          </template>

          <!-- 通用: 插件/通用接入徽标 -->
          <template v-else-if="plat.id === 'generic'">
            <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 22v-5" />
              <path d="M9 8V2" />
              <path d="M15 2v6" />
              <path d="M18 8v5a6 6 0 0 1-12 0V8z" />
            </svg>
          </template>
        </div>

        <!-- 平台名称 -->
        <div class="m26-tab-info">
          <span class="m26-client-tab-name">{{ plat.name }}</span>
        </div>
      </button>
    </nav>

    <!-- 工作区主体：每个平台专属定制视图 -->
    <main class="m26-clients-workspace">
      <!-- 平台专属头部卡片 (Header) - Hermes 采用直接配置，无需冗余横幅 -->
      <section v-if="currentPlatform.id !== 'hermes'" class="m26-client-hero-card">
        <div class="m26-hero-content">
          <div class="m26-hero-avatar" :class="currentPlatform.id">
            <template v-if="currentPlatform.id === 'codex'">
              <svg viewBox="0 0 24 24" width="26" height="26" fill="currentColor">
                <path d="M21.55 10.004a5.416 5.416 0 00-.478-4.501c-1.217-2.09-3.662-3.166-6.05-2.66A5.59 5.59 0 0010.831 1C8.39.995 6.224 2.546 5.473 4.838A5.553 5.553 0 001.76 7.496a5.487 5.487 0 00.691 6.5 5.416 5.416 0 00.477 4.502c1.217 2.09 3.662 3.165 6.05 2.66A5.586 5.586 0 0013.168 23c2.443.006 4.61-1.546 5.361-3.84a5.553 5.553 0 003.715-2.66 5.488 5.488 0 00-.693-6.497v.001zm-8.381 11.558a4.199 4.199 0 01-2.675-.954c.034-.018.093-.05.132-.074l4.44-2.53a.71.71 0 00.364-.623v-6.176l1.877 1.069c.02.01.033.029.036.05v5.115c-.003 2.274-1.87 4.118-4.174 4.123zM4.192 17.78a4.059 4.059 0 01-.498-2.763c.032.02.09.055.131.078l4.44 2.53c.225.13.504.13.73 0l5.42-3.088v2.138a.068.068 0 01-.027.057L9.9 19.288c-1.999 1.136-4.552.46-5.707-1.51h-.001zM3.023 8.216A4.15 4.15 0 015.198 6.41l-.002.151v5.06a.711.711 0 00.364.624l5.42 3.087-1.876 1.07a.067.067 0 01-.063.005l-4.489-2.559c-1.995-1.14-2.679-3.658-1.53-5.63h.001zm15.417 3.54l-5.42-3.088L14.896 7.6a.067.067 0 01.063-.006l4.489 2.557c1.998 1.14 2.683 3.662 1.529 5.633a4.163 4.163 0 01-2.174 1.807V12.38a.71.71 0 00-.363-.623zm1.867-2.773a6.04 6.04 0 00-.132-.078l-4.44-2.53a.731.731 0 00-.729 0l-5.42 3.088V7.325a.068.068 0 01.027-.057L14.1 4.713c2-1.137 4.555-.46 5.707 1.513.487.833.664 1.809.499 2.757h.001zm-11.741 3.81l-1.877-1.068a.065.065 0 01-.036-.051V6.559c.001-2.277 1.873-4.122 4.181-4.12.976 0 1.92.338 2.671.954-.034.018-.092.05-.131.073l-4.44 2.53a.71.71 0 00-.365.623l-.003 6.173v.002zm1.02-2.168L12 9.25l2.414 1.375v2.75L12 14.75l-2.415-1.375v-2.75z" />
              </svg>
            </template>
            <template v-else-if="currentPlatform.id === 'claude-desktop' || currentPlatform.id === 'claude-code'">
              <svg viewBox="0 0 24 24" width="26" height="26" fill="#D97757">
                <path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" />
              </svg>
            </template>
            <template v-else>
              <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 22v-5" /><path d="M9 8V2" /><path d="M15 2v6" /><path d="M18 8v5a6 6 0 0 1-12 0V8z" />
              </svg>
            </template>
          </div>

          <div class="m26-hero-info">
            <div class="flex items-center gap-2">
              <h2 class="m26-hero-title">{{ currentPlatform.name }}</h2>
              <span class="m26-hero-badge">{{ currentPlatform.categoryLabel }}</span>
              <span v-if="currentPlatform.id === 'claude-desktop'" class="m26-hero-badge-mcp">🛡️ MCP 保护中</span>
              <span v-else-if="currentPlatform.id === 'claude-code'" class="m26-hero-badge-cli">Terminal CLI</span>
            </div>
            <p class="m26-hero-desc">{{ currentPlatform.description }}</p>
            <div v-if="currentPlatform.id !== 'generic'" class="m26-hero-path-row">
              <span class="m26-path-label">配置文件:</span>
              <code class="m26-path-code">{{ currentPlatform.configPath }}</code>
              <button class="m26-path-copy-btn" @click="copyText(currentPlatform.configPath)" title="复制文件路径">
                {{ copiedPath ? '已复制' : '复制路径' }}
              </button>
              <button
                v-if="currentPlatform.id === 'claude-desktop'"
                class="m26-path-finder-btn"
                @click="revealInFinder"
                title="在 Finder 中定位配置目录"
              >
                在 Finder 打开
              </button>
            </div>
            <div v-if="finderTip" class="text-xs text-indigo-600 mt-1 font-medium animate-pulse">
              {{ finderTip }}
            </div>
          </div>
        </div>

        <div class="m26-hero-actions">
          <button class="m26-btn-ghost" @click="handleSave(false)">
            保存配置
          </button>
          <button class="m26-btn-primary" @click="handleSave(true)">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="5 3 19 12 5 21 5 3" />
            </svg>
            <span>{{ getLaunchButtonText(currentPlatform.id) }}</span>
          </button>
        </div>
      </section>

      <!-- =================================================================== -->
      <!-- 1. HERMES 专享精简配置视图 (整体重新设计：双栏资源池展示齐全 + Auto 与调度模式紧密结合) -->
      <!-- =================================================================== -->
      <template v-if="currentPlatform.id === 'hermes'">
        <div class="m26-hermes-redesign-container">
          <!-- 1. 顶部控制栏：Auto 智能模型容灾调度 + 调度模式 + 保存并重启 (统一整合面板) -->
          <div class="m26-hermes-top-panel m26-hermes-bottom-panel m26-hermes-auto-card" :class="{ 'auto-enabled': isHermesAutoEnabled }">
            <div class="m26-bottom-panel-main">
              <!-- 左侧流式组合：是否启用AUTO - 开关 - 下拉选择框 - 帮助icon -->
              <div class="m26-panel-left-group">
                <!-- 1. 文案 -->
                <span
                  class="m26-panel-auto-title cursor-pointer select-none"
                  @click="isHermesAutoEnabled = !isHermesAutoEnabled; onToggleHermesAuto()"
                >
                  是否启用AUTO
                </span>

                <!-- 2. 开关 -->
                <label class="m26-switch-label" title="开启/关闭 Auto 智能调度" @click.stop>
                  <input type="checkbox" v-model="isHermesAutoEnabled" @change="onToggleHermesAuto" />
                  <span class="m26-switch-track"></span>
                </label>

                <!-- 3. 紧随其后的调度策略下拉选择框 (精简无解释) -->
                <div class="m26-strategy-dropdown-wrap" :class="{ disabled: !isHermesAutoEnabled }">
                  <!-- 遮罩用于点击外部自动收起 -->
                  <div
                    v-if="isHermesStrategyDropdownOpen"
                    class="m26-popover-backdrop"
                    @click="isHermesStrategyDropdownOpen = false"
                  ></div>

                  <!-- 下拉框触发按钮 -->
                  <button
                    type="button"
                    class="m26-strategy-trigger"
                    :class="{ open: isHermesStrategyDropdownOpen, disabled: !isHermesAutoEnabled }"
                    :disabled="!isHermesAutoEnabled"
                    @click="isHermesStrategyDropdownOpen = !isHermesStrategyDropdownOpen"
                  >
                    <span class="m26-strategy-trigger-text">{{ currentHermesStrategyLabel }}</span>
                    <svg
                      class="m26-strategy-trigger-arrow"
                      :class="{ rotated: isHermesStrategyDropdownOpen }"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                    >
                      <path
                        fill-rule="evenodd"
                        d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
                        clip-rule="evenodd"
                      />
                    </svg>
                  </button>

                  <!-- 自定义下拉选项菜单 (绝对对齐于触发框正下方，精简无解释) -->
                  <div v-if="isHermesStrategyDropdownOpen" class="m26-strategy-options-menu">
                    <div
                      v-for="opt in hermesStrategyOptions"
                      :key="opt.value"
                      class="m26-strategy-option-item"
                      :class="{ selected: hermesStrategy === opt.value }"
                      @click="selectHermesStrategy(opt.value)"
                    >
                      <span class="m26-option-check-icon">
                        <svg
                          v-if="hermesStrategy === opt.value"
                          viewBox="0 0 24 24"
                          width="13"
                          height="13"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2.5"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <polyline points="20 6 9 17 4 12" />
                        </svg>
                      </span>
                      <span class="m26-option-text-main">{{ opt.label }}</span>
                    </div>
                  </div>
                </div>

                <!-- 4. 帮助 icon：点击查看 Auto 作用与各模式区别 -->
                <div class="m26-auto-help-wrapper" @click.stop>
                  <button
                    type="button"
                    class="m26-hint-icon-btn"
                    :class="{ active: showHermesAutoHelp }"
                    title="点击查看 Auto 作用与各模式说明"
                    @click="showHermesAutoHelp = !showHermesAutoHelp"
                  >
                    <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
                      <circle cx="12" cy="12" r="10" />
                      <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
                      <line x1="12" y1="17" x2="12.01" y2="17" />
                    </svg>
                  </button>

                  <!-- 浮层点击背景遮罩 -->
                  <div v-if="showHermesAutoHelp" class="m26-popover-backdrop" @click="showHermesAutoHelp = false"></div>

                  <!-- 弹出说明气泡卡片：显示 Auto 作用与各个模式区别 -->
                  <div v-if="showHermesAutoHelp" class="m26-auto-help-popover">
                    <div class="m26-help-popover-header">
                      <span class="m26-help-popover-title">💡 Auto 调度与模式说明</span>
                      <button type="button" class="m26-help-popover-close" @click="showHermesAutoHelp = false">✕</button>
                    </div>
                    <div class="m26-help-popover-content">
                      <div class="m26-help-block">
                        <div class="m26-help-subtitle">Auto 核心作用</div>
                        <p class="m26-help-text">
                          <strong>开启 Auto：</strong>单渠道故障、超时、429 限流报错时，网关在已勾选模型池中毫秒级自动切换备用模型，保障对话不中断。
                        </p>
                        <p class="m26-help-text text-neutral-500 mt-1">
                          <strong>关闭 Auto：</strong>固定仅使用列表中排在首位的单一模型，不进行跨模型容灾调度。
                        </p>
                      </div>

                      <div class="m26-help-block mt-3 pt-2.5 border-t border-slate-100">
                        <div class="m26-help-subtitle">各个模式区别</div>
                        <div class="m26-help-modes-list">
                          <div class="m26-help-mode-row">
                            <span class="m26-mode-badge quality">智能综合质量优先</span>
                            <span class="m26-mode-desc">综合评估模型梯队与稳定性，优先选用原厂满血高质量通道，遇故障自动顺延。</span>
                          </div>
                          <div class="m26-help-mode-row">
                            <span class="m26-mode-badge latency">最低首字延迟</span>
                            <span class="m26-mode-desc">根据实时测速与历史 TTFT 首字响应时间，动态优先将请求分发至最快的可用渠道。</span>
                          </div>
                          <div class="m26-help-mode-row">
                            <span class="m26-mode-badge balance">轮询负载均衡</span>
                            <span class="m26-mode-desc">在所有选定渠道和模型间均匀轮流分发请求，平摊并发流量，规避单供应商限流。</span>
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 右侧：保存并重启主按钮 (已移除 ~/.hermes/config.yaml) -->
              <div class="m26-panel-right-group">
                <button
                  type="button"
                  class="m26-btn-save-restart"
                  :disabled="isApplyingLocalHermes"
                  @click="applyHermesConfig('restart')"
                >
                  <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" :class="{ 'animate-spin': isApplyingLocalHermes }">
                    <path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67" />
                  </svg>
                  <span>{{ isApplyingLocalHermes ? '正在保存并重启...' : '保存并重启' }}</span>
                </button>
              </div>
            </div>

            <!-- 操作结果反馈条 -->
            <div v-if="hermesActionMessage" class="m26-panel-feedback-bar" :class="hermesActionMessage.type">
              <span class="text-xs font-medium">{{ hermesActionMessage.text }}</span>
              <button type="button" class="m26-feedback-close" @click="hermesActionMessage = null">✕</button>
            </div>
          </div>

          <!-- 2. 双栏资源池 (左供应商、右模型，紧凑高效，双栏自适应内部滚动，一屏全览无外部滚动条) -->
          <div class="m26-hermes-pool-grid">
            <!-- 左栏：供应商池 (紧凑宽度 ~210px，单行项展示，无搜索框) -->
            <div class="m26-pool-panel channel-panel">
              <div class="m26-panel-header">
                <div class="m26-panel-title-row">
                  <div class="flex items-center gap-2">
                    <span class="m26-panel-title">供应商</span>
                    <span class="m26-panel-badge">{{ hermesSelectedChannels.length }}/{{ hermesAvailableChannels.length }}</span>
                  </div>
                  <button
                    type="button"
                    class="m26-action-link"
                    @click="toggleAllHermesChannels"
                  >
                    {{ isAllHermesChannelsSelected ? '取消全选' : '全选所有' }}
                  </button>
                </div>
              </div>

              <!-- 供应商单行紧凑列表 (一行展示: [✓] 书生 7) -->
              <div class="m26-channel-cards-list">
                <div
                  v-if="hermesAvailableChannels.length === 0"
                  class="m26-empty-state"
                >
                  暂无已启用供应商
                </div>

                <div
                  v-for="ch in hermesAvailableChannels"
                  :key="ch.name"
                  class="m26-channel-select-card m26-channel-chip"
                  :class="{ selected: isHermesChannelSelected(ch.name) }"
                  @click="toggleHermesChannel(ch.name)"
                  :title="`${ch.name} (${getChannelModelCount(ch)} 款模型)`"
                >
                  <div class="m26-channel-select-card-left">
                    <div class="m26-mini-checkbox" :class="{ checked: isHermesChannelSelected(ch.name) }">
                      <span v-if="isHermesChannelSelected(ch.name)">✓</span>
                    </div>
                    <span class="m26-channel-name truncate" :title="ch.name">{{ ch.name }}</span>
                  </div>
                  <span class="m26-channel-count-pill shrink-0">{{ getChannelModelCount(ch) }}</span>
                </div>
              </div>
            </div>

            <!-- 右栏：调度模型池 (多分类 Tabs，无搜索框) -->
            <div class="m26-pool-panel model-panel">
              <div class="m26-panel-header">
                <div class="m26-panel-title-row">
                  <div class="flex items-center gap-2 min-w-0">
                    <span class="m26-panel-title">调度模型池</span>
                    <span v-if="selectedHermesModelFamily !== 'all'" class="text-xs font-semibold text-indigo-600 bg-indigo-50 px-1.5 py-0.5 rounded">
                      {{ MODEL_FAMILY_LABELS[selectedHermesModelFamily] || selectedHermesModelFamily }}
                    </span>
                    <span class="m26-panel-badge">
                      {{ selectedHermesModelFamily === 'all'
                        ? `${effectiveHermesAllowedModels.length}/${hermesMergedModels.length}`
                        : `${currentFilterSelectedCount}/${filteredHermesModels.length}` }}
                    </span>
                  </div>
                  <button
                    type="button"
                    class="m26-action-link"
                    :disabled="hermesSelectedChannels.length === 0"
                    @click="toggleAllHermesModels"
                  >
                    {{ isAllHermesModelsSelected ? (selectedHermesModelFamily === 'all' ? '取消全选' : '取消此类') : (selectedHermesModelFamily === 'all' ? '全选所有' : '全选此类') }}
                  </button>
                </div>

                <!-- 分类 Tabs (纯单行左右平滑滚动，带方向导航与滚轮响应) -->
                <!-- 分类 Tabs (直接鼠标左右滑动/滚轮滑动) -->
                <div
                  ref="hermesTabsScrollRef"
                  class="m26-family-tabs-scroll"
                  @wheel="onHermesTabsWheel"
                >
                  <button
                    v-for="tab in hermesModelFamilyTabs"
                    :key="tab.key"
                    type="button"
                    class="m26-family-chip"
                    :class="{ active: selectedHermesModelFamily === tab.key }"
                    @click="selectedHermesModelFamily = tab.key"
                  >
                    <span>{{ tab.label }}</span>
                    <span class="m26-chip-count">{{ tab.count }}</span>
                  </button>
                </div>
              </div>

              <!-- 模型列表 (展示齐全) -->
              <div class="m26-model-cards-scroll">
                <div
                  v-if="filteredHermesModels.length === 0"
                  class="m26-empty-state"
                >
                  {{ hermesSelectedChannels.length === 0 ? '请先在左侧勾选供应商' : '暂无可用模型' }}
                </div>

                <div
                  v-for="item in filteredHermesModels"
                  :key="item.name"
                  class="m26-model-select-card"
                  :class="{ selected: isHermesModelSelected(item.name) }"
                  @click="toggleHermesModel(item.name)"
                >
                  <div class="flex items-center gap-2.5 min-w-0">
                    <div class="m26-mini-checkbox" :class="{ checked: isHermesModelSelected(item.name) }">
                      <span v-if="isHermesModelSelected(item.name)">✓</span>
                    </div>
                    <div class="flex flex-col min-w-0">
                      <div class="flex items-center gap-2">
                        <span class="m26-model-title font-mono" :title="item.name">{{ item.name }}</span>
                        <span v-if="item.rawNames.length > 1" class="m26-merged-pill">
                          归并 {{ item.rawNames.length }} 源
                        </span>
                      </div>
                      <div v-if="item.aliases.length > 0" class="m26-model-aliases" :title="item.rawNames.join(', ')">
                        别名: {{ item.aliases.slice(0, 2).join(', ') }}{{ item.aliases.length > 2 ? '...' : '' }}
                      </div>
                    </div>
                  </div>

                  <div class="flex items-center gap-2 shrink-0">
                    <span class="m26-provider-pill" :title="item.providers.join(', ')">
                      {{ item.providers.slice(0, 2).join(' · ') }}{{ item.providers.length > 2 ? ' 等' : '' }} ({{ item.providers.length }}家)
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </div>
</div>
      </template>

      <!-- =================================================================== -->
      <!-- 2. CODEX 定制视图 (OpenAI 协议编程工具与终端助手)                       -->
      <!-- =================================================================== -->
      <template v-else-if="currentPlatform.id === 'codex'">
        <!-- 接入模式轻量分段切换 -->
        <section class="m26-client-mode-strip">
          <div class="m26-mode-strip-left">
            <span class="text-sm font-semibold text-neutral-800">接入方式</span>
            <span class="m26-mode-strip-desc">选择直接连向上游供应商，或由 HeiGate 本地网关进行统一调度</span>
          </div>

          <div class="m26-mode-segmented">
            <button
              class="m26-mode-btn"
              :class="{ active: currentPlatform.activeMode === 'direct' }"
              @click="switchMode('direct')"
            >
              <span>⚡️ 直连供应商</span>
            </button>
            <button
              class="m26-mode-btn"
              :class="{ active: currentPlatform.activeMode === 'gateway' }"
              @click="switchMode('gateway')"
            >
              <span>🛡 走 HeiGate 网关</span>
            </button>
          </div>
        </section>

        <!-- 模式 A：Codex 直连 OpenAI 兼容供应商 -->
        <section v-if="currentPlatform.activeMode === 'direct'" class="m26-client-config-card">
          <div class="m26-config-head">
            <div class="m26-config-title">Codex 直连 OpenAI 兼容供应商</div>
            <div class="m26-config-subtitle">复用 HeiGate 已配置的 OpenAI 格式供应商，直连请求不经过网关</div>
          </div>

          <div class="m26-config-form">
            <div class="m26-form-group">
              <label class="m26-form-label">选择供应商渠道</label>
              <div class="m26-select-wrap">
                <select
                  class="m26-select"
                  :value="currentPlatform.direct.channelName"
                  @change="selectDirectChannel(($event.target as HTMLSelectElement).value)"
                >
                  <option value="" disabled>-- 请选择 OpenAI 协议渠道 --</option>
                  <option
                    v-for="chan in compatibleChannels"
                    :key="chan.name"
                    :value="chan.name"
                  >
                    {{ chan.name }} ({{ chan.models }} 个可用模型)
                  </option>
                </select>
              </div>
            </div>

            <!-- 直连模型快捷选择 -->
            <div class="m26-form-group">
              <div class="flex items-center justify-between">
                <label class="m26-form-label">指定模型 (Model Name)</label>
                <div class="m26-quick-models">
                  <span class="text-xs text-neutral-400 mr-1">快捷填入:</span>
                  <button type="button" class="m26-chip-btn" @click="quickSelectModel('gpt-4o', true)">GPT-4o</button>
                  <button type="button" class="m26-chip-btn" @click="quickSelectModel('o3-mini', true)">o3-mini</button>
                  <button type="button" class="m26-chip-btn" @click="quickSelectModel('deepseek-chat', true)">DeepSeek V3</button>
                </div>
              </div>
              <input
                v-model="currentPlatform.direct.model"
                type="text"
                class="m26-input"
                list="codex-models-list"
                placeholder="例如 gpt-4o 或 deepseek-chat"
              />
              <datalist id="codex-models-list">
                <option v-for="m in currentChannelModels" :key="m" :value="m" />
              </datalist>
            </div>

            <!-- 连通性测试 -->
            <div class="m26-test-row">
              <button
                class="m26-btn-test"
                :disabled="!currentPlatform.direct.channelName || testState['codex'] === 'testing'"
                @click="runTestConnection"
              >
                <span>{{ testState['codex'] === 'testing' ? '正在探测连通性...' : '测试直连网络与密钥' }}</span>
              </button>
              <span v-if="testMessage['codex']" class="m26-test-result-msg" :class="testState['codex']">
                {{ testMessage['codex'] }}
              </span>
            </div>
          </div>
        </section>

        <!-- 模式 B：Codex 走 HeiGate 网关 -->
        <section v-else class="m26-client-config-card">
          <div class="m26-config-head">
            <div class="m26-config-title">Codex 专属网关调度策略</div>
            <div class="m26-config-subtitle">通过本地网关代理 Codex 请求，提供多渠道故障容灾避让</div>
          </div>

          <div class="m26-config-form">
            <div class="m26-form-group">
              <label class="m26-form-label">调度分发算法</label>
              <div class="m26-strategy-cards">
                <div
                  class="m26-strategy-card"
                  :class="{ selected: currentPlatform.gateway.strategy === 'smart_quality' }"
                  @click="currentPlatform.gateway.strategy = 'smart_quality'"
                >
                  <div class="m26-strategy-title">智能综合质量</div>
                  <div class="m26-strategy-desc">动态优选当前最健康的可用渠道</div>
                </div>
                <div
                  class="m26-strategy-card"
                  :class="{ selected: currentPlatform.gateway.strategy === 'latency' }"
                  @click="currentPlatform.gateway.strategy = 'latency'"
                >
                  <div class="m26-strategy-title">最低延迟优先</div>
                  <div class="m26-strategy-desc">针对代码补全场景极速响应</div>
                </div>
                <div
                  class="m26-strategy-card"
                  :class="{ selected: currentPlatform.gateway.strategy === 'round_robin' }"
                  @click="currentPlatform.gateway.strategy = 'round_robin'"
                >
                  <div class="m26-strategy-title">轮询负载均衡</div>
                  <div class="m26-strategy-desc">候选渠道间均衡负载轮流分发</div>
                </div>
              </div>
            </div>

            <!-- 可用供应商池 -->
            <div class="m26-form-group">
              <label class="m26-form-label">参与 Codex 调度的供应商渠道</label>
              <div class="m26-checkbox-grid">
                <label
                  v-for="chan in compatibleChannels"
                  :key="chan.name"
                  class="m26-checkbox-item"
                  :class="{ checked: currentPlatform.gateway.allowedChannels.includes(chan.name) }"
                >
                  <input
                    type="checkbox"
                    class="sr-only"
                    :checked="currentPlatform.gateway.allowedChannels.includes(chan.name)"
                    @change="toggleGatewayChannel(chan.name)"
                  />
                  <span class="m26-checkbox-box"></span>
                  <span class="m26-checkbox-text">{{ chan.name }}</span>
                  <span class="m26-checkbox-proto">{{ chan.protocol }}</span>
                </label>
              </div>
            </div>
          </div>
        </section>

        <!-- Codex 专属亮点：CLI 环境变量快捷复制 -->
        <section class="m26-cli-env-card">
          <div class="m26-cli-env-head">
            <span class="m26-cli-env-title">💻 Codex 终端环境命令 (一行快速生效)</span>
            <button class="m26-btn-ghost-sm" @click="copyEnvScript('codex')">
              {{ copiedEnv ? '已复制环境变量' : '复制终端 export 命令' }}
            </button>
          </div>
          <pre class="m26-cli-code"><code>export OPENAI_BASE_URL="{{ gatewayBaseUrl }}"
export OPENAI_API_KEY="{{ gatewayKey || 'heigate-local-gateway-key' }}"</code></pre>
        </section>

        <!-- 配置文件保护与 Diff -->
        <section class="m26-config-guard-card">
          <div class="m26-guard-header" @click="showDiff = !showDiff">
            <div class="flex items-center gap-2">
              <svg class="text-emerald-500" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
              </svg>
              <span class="m26-guard-title">Codex 配置保护与差量合并</span>
            </div>
            <span class="m26-guard-toggle">
              {{ showDiff ? '收起配置详情 ▲' : '查看写入预览 ▼' }}
            </span>
          </div>
          <div v-if="showDiff" class="m26-diff-box">
            <pre class="m26-diff-code"><code>{{ diffPreview }}</code></pre>
          </div>
        </section>
      </template>

      <!-- =================================================================== -->
      <!-- 3. CLAUDE DESKTOP 定制视图 (macOS 原生客户端 · 核心 MCP 保护)         -->
      <!-- =================================================================== -->
      <template v-else-if="currentPlatform.id === 'claude-desktop'">
        <!-- Claude Desktop 专属重点：MCP 工具安全防护卡片 -->
        <section class="m26-mcp-banner-card">
          <div class="m26-mcp-banner-icon">
            <svg viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
              <polyline points="9 12 11 14 15 10" />
            </svg>
          </div>
          <div class="m26-mcp-banner-body">
            <div class="m26-mcp-banner-title">🛡️ MCP (Model Context Protocol) 插件安全防护机制已启用</div>
            <div class="m26-mcp-banner-desc">
              已检测到 macOS 本机配置文件。HeiGate 仅差量更新 API 路由端点与模型，<strong>绝对保留您现有配置的所有 mcpServers 插件服务</strong>，写入前自动生成 <code>.bak</code> 备份文件。
            </div>
          </div>
          <button class="m26-mcp-finder-btn" @click="revealInFinder">
            在 Finder 中定位
          </button>
        </section>

        <!-- 接入方式分段切换 -->
        <section class="m26-client-mode-strip">
          <div class="m26-mode-strip-left">
            <span class="text-sm font-semibold text-neutral-800">接入模式</span>
            <span class="m26-mode-strip-desc">选择直连 Anthropic 官方/转发渠道，或经由 HeiGate 网关统一代理</span>
          </div>

          <div class="m26-mode-segmented">
            <button
              class="m26-mode-btn"
              :class="{ active: currentPlatform.activeMode === 'direct' }"
              @click="switchMode('direct')"
            >
              <span>⚡️ 直连 Anthropic 渠道</span>
            </button>
            <button
              class="m26-mode-btn"
              :class="{ active: currentPlatform.activeMode === 'gateway' }"
              @click="switchMode('gateway')"
            >
              <span>🛡 走 HeiGate 网关</span>
            </button>
          </div>
        </section>

        <!-- 直连配置卡片 -->
        <section v-if="currentPlatform.activeMode === 'direct'" class="m26-client-config-card">
          <div class="m26-config-head">
            <div class="m26-config-title">Claude Desktop 直连设置</div>
            <div class="m26-config-subtitle">选择 Anthropic 协议渠道与官方 Key，直接写入本地配置</div>
          </div>

          <div class="m26-config-form">
            <div class="m26-form-group">
              <label class="m26-form-label">选择 Anthropic 协议渠道</label>
              <div class="m26-select-wrap">
                <select
                  class="m26-select"
                  :value="currentPlatform.direct.channelName"
                  @change="selectDirectChannel(($event.target as HTMLSelectElement).value)"
                >
                  <option value="" disabled>-- 请选择 Anthropic 协议供应商 --</option>
                  <option
                    v-for="chan in compatibleChannels"
                    :key="chan.name"
                    :value="chan.name"
                  >
                    {{ chan.name }} ({{ chan.models }} 个模型)
                  </option>
                </select>
              </div>
            </div>

            <!-- 模型选择 -->
            <div class="m26-form-group">
              <div class="flex items-center justify-between">
                <label class="m26-form-label">主力模型 (Default Model)</label>
                <div class="m26-quick-models">
                  <span class="text-xs text-neutral-400 mr-1">快捷填入:</span>
                  <button type="button" class="m26-chip-btn" @click="quickSelectModel('claude-3-7-sonnet-latest', true)">Claude 3.7</button>
                  <button type="button" class="m26-chip-btn" @click="quickSelectModel('claude-3-5-sonnet-20241022', true)">Claude 3.5 Sonnet</button>
                </div>
              </div>
              <input
                v-model="currentPlatform.direct.model"
                type="text"
                class="m26-input"
                placeholder="claude-3-7-sonnet-latest"
              />
            </div>

            <!-- 连通性测试 -->
            <div class="m26-test-row">
              <button
                class="m26-btn-test"
                :disabled="!currentPlatform.direct.channelName || testState['claude-desktop'] === 'testing'"
                @click="runTestConnection"
              >
                <span>{{ testState['claude-desktop'] === 'testing' ? '正在探测...' : '测试直连网络' }}</span>
              </button>
              <span v-if="testMessage['claude-desktop']" class="m26-test-result-msg" :class="testState['claude-desktop']">
                {{ testMessage['claude-desktop'] }}
              </span>
            </div>
          </div>
        </section>

        <!-- 网关配置卡片 -->
        <section v-else class="m26-client-config-card">
          <div class="m26-config-head">
            <div class="m26-config-title">Claude Desktop 网关容灾调度策略</div>
            <div class="m26-config-subtitle">配置此客户端在多渠道间的调度算法与备用渠道</div>
          </div>

          <div class="m26-config-form">
            <div class="m26-form-group">
              <label class="m26-form-label">调度分发算法</label>
              <div class="m26-strategy-cards">
                <div
                  class="m26-strategy-card"
                  :class="{ selected: currentPlatform.gateway.strategy === 'smart_quality' }"
                  @click="currentPlatform.gateway.strategy = 'smart_quality'"
                >
                  <div class="m26-strategy-title">智能综合质量</div>
                  <div class="m26-strategy-desc">优先保障对话流畅度与可用性</div>
                </div>
                <div
                  class="m26-strategy-card"
                  :class="{ selected: currentPlatform.gateway.strategy === 'latency' }"
                  @click="currentPlatform.gateway.strategy = 'latency'"
                >
                  <div class="m26-strategy-title">最低延迟优先</div>
                  <div class="m26-strategy-desc">实时选取首字耗时最快的节点</div>
                </div>
                <div
                  class="m26-strategy-card"
                  :class="{ selected: currentPlatform.gateway.strategy === 'round_robin' }"
                  @click="currentPlatform.gateway.strategy = 'round_robin'"
                >
                  <div class="m26-strategy-title">轮询负载均衡</div>
                  <div class="m26-strategy-desc">在可用渠道中均匀调度流量</div>
                </div>
              </div>
            </div>

            <!-- 可用渠道池 -->
            <div class="m26-form-group">
              <label class="m26-form-label">参与此平台的 Anthropic 渠道</label>
              <div class="m26-checkbox-grid">
                <label
                  v-for="chan in compatibleChannels"
                  :key="chan.name"
                  class="m26-checkbox-item"
                  :class="{ checked: currentPlatform.gateway.allowedChannels.includes(chan.name) }"
                >
                  <input
                    type="checkbox"
                    class="sr-only"
                    :checked="currentPlatform.gateway.allowedChannels.includes(chan.name)"
                    @change="toggleGatewayChannel(chan.name)"
                  />
                  <span class="m26-checkbox-box"></span>
                  <span class="m26-checkbox-text">{{ chan.name }}</span>
                  <span class="m26-checkbox-proto">{{ chan.protocol }}</span>
                </label>
              </div>
            </div>
          </div>
        </section>

        <!-- Diff 预览 -->
        <section class="m26-config-guard-card">
          <div class="m26-guard-header" @click="showDiff = !showDiff">
            <div class="flex items-center gap-2">
              <svg class="text-emerald-500" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
              </svg>
              <span class="m26-guard-title">配置保护与 MCP 节点保留确认</span>
            </div>
            <span class="m26-guard-toggle">
              {{ showDiff ? '收起配置详情 ▲' : '查看写入预览 ▼' }}
            </span>
          </div>
          <div v-if="showDiff" class="m26-diff-box">
            <pre class="m26-diff-code"><code>{{ diffPreview }}</code></pre>
          </div>
        </section>
      </template>

      <!-- =================================================================== -->
      <!-- 4. CLAUDE CODE 定制视图 (终端 CLI · 双模型架构 · 环境变量脚本)           -->
      <!-- =================================================================== -->
      <template v-else-if="currentPlatform.id === 'claude-code'">
        <!-- Claude Code 专属终端环境脚本卡片 -->
        <section class="m26-cli-env-card">
          <div class="m26-cli-env-head">
            <div class="flex items-center gap-2">
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="4 17 10 11 4 5" />
                <line x1="12" y1="19" x2="20" y2="19" />
              </svg>
              <span class="m26-cli-env-title">Claude Code 终端环境变量（一键配置）</span>
            </div>
            <button class="m26-btn-ghost-sm" @click="copyEnvScript('claude-code')">
              {{ copiedEnv ? '已复制环境变量' : '复制终端 export 命令' }}
            </button>
          </div>
          <pre class="m26-cli-code"><code>export ANTHROPIC_BASE_URL="{{ gatewayBaseUrl.replace('/v1', '') }}"
export ANTHROPIC_API_KEY="{{ gatewayKey || 'heigate-local-gateway-key' }}"</code></pre>
          <div class="m26-cli-env-tip">
            在终端粘贴执行或写入 <code>~/.zshrc</code>，在任意项目目录下输入 <code>claude</code> 即可直接享受本地网关调度加速。
          </div>
        </section>

        <!-- 模式切换 -->
        <section class="m26-client-mode-strip">
          <div class="m26-mode-strip-left">
            <span class="text-sm font-semibold text-neutral-800">接入模式</span>
            <span class="m26-mode-strip-desc">选择直连上游或通过网关调度</span>
          </div>

          <div class="m26-mode-segmented">
            <button
              class="m26-mode-btn"
              :class="{ active: currentPlatform.activeMode === 'direct' }"
              @click="switchMode('direct')"
            >
              <span>⚡️ 直连 Anthropic 渠道</span>
            </button>
            <button
              class="m26-mode-btn"
              :class="{ active: currentPlatform.activeMode === 'gateway' }"
              @click="switchMode('gateway')"
            >
              <span>🛡 走 HeiGate 网关</span>
            </button>
          </div>
        </section>

        <!-- Claude Code 专属核心：双模型架构设置 (Dual Model Config) -->
        <section class="m26-client-config-card">
          <div class="m26-config-head">
            <div class="m26-config-title">Claude Code 双模型智能架构设置</div>
            <div class="m26-config-subtitle">Claude Code 官方支持主控复杂推理模型与次级极速响应模型分离</div>
          </div>

          <div class="m26-config-form">
            <!-- 供应商选择 (直连模式下) -->
            <div v-if="currentPlatform.activeMode === 'direct'" class="m26-form-group">
              <label class="m26-form-label">指定直连供应商渠道</label>
              <div class="m26-select-wrap">
                <select
                  class="m26-select"
                  :value="currentPlatform.direct.channelName"
                  @change="selectDirectChannel(($event.target as HTMLSelectElement).value)"
                >
                  <option value="" disabled>-- 请选择兼容供应商 --</option>
                  <option
                    v-for="chan in compatibleChannels"
                    :key="chan.name"
                    :value="chan.name"
                  >
                    {{ chan.name }} ({{ chan.models }} 个可用模型)
                  </option>
                </select>
              </div>
            </div>

            <!-- 主模型与次级模型双输入 -->
            <div class="m26-form-grid">
              <div class="m26-form-group">
                <div class="flex items-center justify-between">
                  <label class="m26-form-label">主力主脑模型 (Primary Model)</label>
                  <button type="button" class="m26-chip-btn" @click="quickSelectModel('claude-3-7-sonnet-latest', currentPlatform.activeMode === 'direct')">Claude 3.7</button>
                </div>
                <input
                  v-if="currentPlatform.activeMode === 'direct'"
                  v-model="currentPlatform.direct.model"
                  type="text"
                  class="m26-input"
                  placeholder="claude-3-7-sonnet-latest"
                />
                <input
                  v-else
                  v-model="currentPlatform.gateway.model"
                  type="text"
                  class="m26-input"
                  placeholder="claude-3-7-sonnet-latest"
                />
                <span class="m26-field-desc">用于复杂任务规划、代码架构生成与高难度重构</span>
              </div>

              <div class="m26-form-group">
                <div class="flex items-center justify-between">
                  <label class="m26-form-label">次级极速模型 (Small Fast Model)</label>
                  <button type="button" class="m26-chip-btn" @click="quickSelectSmallModel('claude-3-5-haiku-latest')">Haiku 3.5</button>
                </div>
                <input
                  v-model="currentPlatform.direct.smallModel"
                  type="text"
                  class="m26-input"
                  placeholder="claude-3-5-haiku-latest"
                />
                <span class="m26-field-desc">用于快速行补全、文件查找与小型指令执行</span>
              </div>
            </div>

            <!-- 网关模式下的算法与渠道 -->
            <template v-if="currentPlatform.activeMode === 'gateway'">
              <div class="m26-form-group">
                <label class="m26-form-label">网关分发调度策略</label>
                <div class="m26-strategy-cards">
                  <div
                    class="m26-strategy-card"
                    :class="{ selected: currentPlatform.gateway.strategy === 'latency' }"
                    @click="currentPlatform.gateway.strategy = 'latency'"
                  >
                    <div class="m26-strategy-title">⚡️ 最低延迟优先（推荐）</div>
                    <div class="m26-strategy-desc">专为 CLI 终端交互优化的响应速度</div>
                  </div>
                  <div
                    class="m26-strategy-card"
                    :class="{ selected: currentPlatform.gateway.strategy === 'smart_quality' }"
                    @click="currentPlatform.gateway.strategy = 'smart_quality'"
                  >
                    <div class="m26-strategy-title">智能综合质量</div>
                    <div class="m26-strategy-desc">以最高模型输出准确度为核心</div>
                  </div>
                  <div
                    class="m26-strategy-card"
                    :class="{ selected: currentPlatform.gateway.strategy === 'round_robin' }"
                    @click="currentPlatform.gateway.strategy = 'round_robin'"
                  >
                    <div class="m26-strategy-title">轮询负载均衡</div>
                    <div class="m26-strategy-desc">在渠道池间平均分配终端流量</div>
                  </div>
                </div>
              </div>
            </template>
          </div>
        </section>

        <!-- 差量预览 -->
        <section class="m26-config-guard-card">
          <div class="m26-guard-header" @click="showDiff = !showDiff">
            <span class="m26-guard-title">CLI 配置保护与 ~/.claude.json 差量预览</span>
            <span class="m26-guard-toggle">{{ showDiff ? '收起 ▲' : '展开 ▼' }}</span>
          </div>
          <div v-if="showDiff" class="m26-diff-box">
            <pre class="m26-diff-code"><code>{{ diffPreview }}</code></pre>
          </div>
        </section>
      </template>

      <!-- =================================================================== -->
      <!-- 5. 通用接入 (Universal Gateway for Cursor, Windsurf, Cline, Python)  -->
      <!-- =================================================================== -->
      <template v-else>
        <!-- 网关核心凭证与端点 -->
        <GatewayAccessSettings
          :gateway-base-url="gatewayBaseUrl"
          :gateway-api-key="gatewayKey || ''"
          :masked-overview-api-key="maskedGenericApiKey"
          :show-api-key="showGenericApiKey"
          @copy-url="copyText(gatewayBaseUrl)"
          @copy-key="copyText(gatewayKey || '', true)"
          @update:show-api-key="(val) => (showGenericApiKey = val)"
        />

        <!-- 全局路由分发策略 -->
        <RoutingSettings
          :routing-strategy="routingStrategy || 'smart_quality'"
          @select="(strategy) => emit('update-routing-strategy', strategy)"
        />

        <!-- 第三方工具快速接入指引 -->
        <section class="m26-generic-guide-card">
          <div class="m26-guide-header">
            <div class="flex items-center gap-2">
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="16" x2="12" y2="12" />
                <line x1="12" y1="8" x2="12.01" y2="8" />
              </svg>
              <span class="font-semibold text-neutral-800 text-sm">常见客户端快速接入指引</span>
            </div>
          </div>
          <div class="m26-guide-grid">
            <div class="m26-guide-item">
              <div class="m26-guide-app-title">Cursor / Windsurf</div>
              <div class="m26-guide-app-desc">在设置中的 OpenAI API Key 处填入：</div>
              <ul class="m26-guide-list">
                <li><span class="m26-guide-k">Base URL:</span> <code>{{ gatewayBaseUrl }}</code></li>
                <li><span class="m26-guide-k">API Key:</span> <code>{{ maskedGenericApiKey }}</code></li>
                <li><span class="m26-guide-k">Model:</span> <code>填入渠道中支持的任意模型名</code></li>
              </ul>
            </div>
            <div class="m26-guide-item">
              <div class="m26-guide-app-title">Cline / Roo Code / Continue</div>
              <div class="m26-guide-app-desc">选择 OpenAI-Compatible 或 Anthropic 兼容模式：</div>
              <ul class="m26-guide-list">
                <li><span class="m26-guide-k">Base URL:</span> <code>{{ gatewayBaseUrl }}</code></li>
                <li><span class="m26-guide-k">API Key:</span> <code>{{ maskedGenericApiKey }}</code></li>
              </ul>
            </div>
            <div class="m26-guide-item">
              <div class="m26-guide-app-title">Python / OpenAI SDK</div>
              <div class="m26-guide-app-desc">使用官方 SDK 初始化本地端点：</div>
              <pre class="m26-guide-code"><code>client = OpenAI(
  base_url="{{ gatewayBaseUrl }}",
  api_key="{{ gatewayKey || 'heigate-secret' }}"
)</code></pre>
            </div>
            <div class="m26-guide-item">
              <div class="m26-guide-app-title">cURL 终端快速测试</div>
              <div class="m26-guide-app-desc">直接向本地网关端口发起测试请求：</div>
              <pre class="m26-guide-code"><code>curl {{ gatewayBaseUrl }}/chat/completions \
  -H "Authorization: Bearer {{ gatewayKey || 'heigate-secret' }}" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-3-7-sonnet-latest","messages":[{"role":"user","content":"ping"}]}'</code></pre>
            </div>
          </div>
        </section>
      </template>
    </main>
  </div>
</template>

<style scoped>
.m26-clients-layout {
  display: flex;
  flex-direction: column;
  height: 100%;
  width: 100%;
  min-height: 0;
  overflow: hidden;
  background: transparent;
  gap: 12px;
}

/* 顶部横向 5 联独立客户端卡片 (5 Separate Bento Cards) */
.m26-clients-nav-deck {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  gap: 10px;
  width: 100%;
  flex-shrink: 0;
  background: transparent;
  border: none;
  padding: 2px 1px 4px 1px;
}

.m26-client-tab-card {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 14px;
  border-radius: 12px;
  border: 1px solid rgba(0, 0, 0, 0.08);
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  cursor: pointer;
  transition: background-color 0.16s ease, border-color 0.16s ease, box-shadow 0.16s ease;
  text-align: left;
  user-select: none;
  position: relative;
  min-width: 0;
  outline: none;
}

/* 未激活卡片在鼠标悬停时：背景纯白、边框微加深、柔和轻微阴影，绝不抖动或丢失边框 */
.m26-client-tab-card:hover:not(.active) {
  background: #ffffff;
  border-color: rgba(0, 0, 0, 0.16);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.05);
}

.m26-client-tab-card:active:not(.active) {
  background: rgba(0, 0, 0, 0.02);
}

/* 激活状态：独立浮起卡片，清晰 1px 强调边框 + 柔和阴影，任何悬浮状态下边框都绝不丢失 */
.m26-client-tab-card.active {
  background: #ffffff;
  border-color: var(--m26-accent, #4f46e5);
  box-shadow: 0 0 0 1px var(--m26-accent, #4f46e5), 0 2px 8px rgba(79, 70, 229, 0.12);
}

.m26-client-tab-card.active:hover {
  background: #ffffff;
  border-color: var(--m26-accent, #4f46e5);
  box-shadow: 0 0 0 1px var(--m26-accent, #4f46e5), 0 3px 10px rgba(79, 70, 229, 0.16);
}

/* 图标容器 */
.m26-tab-logo-box {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  position: relative;
  transition: transform 0.2s ease;
}

.m26-client-tab-card:hover .m26-tab-logo-box {
  transform: scale(1.05);
}

.m26-tab-logo-box.claude-desktop,
.m26-tab-logo-box.claude-code {
  color: #D97757;
  background: #fff8f5;
  border: 1px solid #ffedd5;
}

.m26-tab-logo-box.codex {
  color: #10a37f;
  background: #ecfdf5;
  border: 1px solid #d1fae5;
}

.m26-tab-logo-box.hermes {
  background: #f8fafc;
  border: 1px solid #e2e8f0;
}

.m26-tab-logo-box.generic {
  color: #6366f1;
  background: #eef2ff;
  border: 1px solid #e0e7ff;
}

.m26-glyph-badge {
  position: absolute;
  right: -2px;
  bottom: -2px;
  width: 12px;
  height: 12px;
  border-radius: 3px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.08);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08);
  pointer-events: none;
  z-index: 2;
}

.m26-glyph-badge.desktop {
  color: #9a3412;
}

.m26-glyph-badge.terminal {
  color: #18181b;
  background: #f4f4f5;
}

.m26-deck-hermes-img {
  width: 22px;
  height: 22px;
  object-fit: cover;
  border-radius: 50%;
  display: block;
}

.m26-tab-info {
  display: flex;
  align-items: center;
  min-width: 0;
  flex: 1;
}

.m26-client-tab-name {
  font-size: 13.5px;
  font-weight: 600;
  color: var(--m26-text, #18181b);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.m26-client-tab-card.active .m26-client-tab-name {
  color: #4338ca;
  font-weight: 600;
}

/* 主工作区 */
.m26-clients-workspace {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 0 2px 20px 2px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.m26-clients-workspace:has(.m26-hermes-redesign-container) {
  overflow-y: hidden;
  padding-bottom: 14px;
  gap: 8px;
}

/* 顶部概览卡片 */
.m26-client-hero-card {
  padding: 14px 18px;
  border-radius: 12px;
  border: 1px solid var(--m26-border);
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.m26-hero-content {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
}

.m26-hero-avatar {
  width: 42px;
  height: 42px;
  border-radius: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.m26-hero-avatar.claude-desktop,
.m26-hero-avatar.claude-code {
  color: #D97757;
  background: #fff8f5;
  border: 1px solid #fed7aa;
}

.m26-hero-avatar.codex {
  color: #10a37f;
  background: #ecfdf5;
  border: 1px solid #d1fae5;
}

.m26-hero-avatar.hermes {
  background: #f8fafc;
  border: 1px solid #e2e8f0;
}

.m26-hero-avatar.generic {
  color: #6366f1;
  background: #eef2ff;
  border: 1px solid #e0e7ff;
}

.m26-hero-hermes-img {
  width: 34px;
  height: 34px;
  object-fit: contain;
  border-radius: 50%;
}

.m26-hero-title {
  font-size: 15.5px;
  font-weight: 650;
  color: var(--m26-text);
  margin: 0;
}

.m26-hero-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.05);
  color: var(--m26-text-secondary);
}

.m26-hero-badge-special {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: #f5f3ff;
  color: #7c3aed;
  border: 1px solid #ede9fe;
}

.m26-hero-badge-mcp {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: #ecfdf5;
  color: #059669;
  border: 1px solid #d1fae5;
}

.m26-hero-badge-cli {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: #f4f4f5;
  color: #18181b;
  border: 1px solid #e4e4e7;
}

.m26-hero-desc {
  font-size: 12px;
  color: var(--m26-text-secondary);
  margin: 3px 0 6px 0;
}

.m26-hero-path-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11.5px;
}

.m26-path-label {
  color: var(--m26-text-muted);
}

.m26-path-code {
  background: rgba(0, 0, 0, 0.04);
  color: var(--m26-text);
  padding: 2px 6px;
  border-radius: 4px;
  font-family: monospace;
  border: 1px solid rgba(0, 0, 0, 0.05);
}

.m26-path-copy-btn,
.m26-path-finder-btn {
  font-size: 11px;
  color: var(--m26-accent);
  background: transparent;
  border: none;
  cursor: pointer;
  font-weight: 500;
  padding: 0;
}

.m26-path-copy-btn:hover,
.m26-path-finder-btn:hover {
  text-decoration: underline;
}

.m26-hero-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.m26-btn-ghost {
  height: 32px;
  padding: 0 14px;
  font-size: 12px;
  font-weight: 500;
  color: var(--m26-text);
  background: #ffffff;
  border: 1px solid var(--m26-border-bright);
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s ease;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.m26-btn-ghost:hover {
  background: #f8fafc;
  border-color: rgba(0, 0, 0, 0.2);
}

.m26-btn-ghost-sm {
  height: 26px;
  padding: 0 10px;
  font-size: 11px;
  font-weight: 500;
  color: var(--m26-accent);
  background: #ffffff;
  border: 1px solid var(--m26-border-bright);
  border-radius: 6px;
  cursor: pointer;
}

.m26-btn-ghost-sm:hover {
  background: #f8fafc;
}

.m26-btn-primary {
  height: 32px;
  padding: 0 16px;
  font-size: 12px;
  font-weight: 500;
  color: #ffffff;
  background: var(--m26-accent);
  border: 1px solid rgba(79, 70, 229, 0.3);
  border-radius: 8px;
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 6px;
  transition: all 0.15s ease;
  box-shadow: 0 2px 8px rgba(79, 70, 229, 0.25);
  white-space: nowrap;
}

.m26-btn-primary:hover {
  background: #4338ca;
}

/* 模式条 (Mode Strip) */
.m26-client-mode-strip {
  padding: 10px 16px;
  border-radius: 12px;
  border: 1px solid var(--m26-border);
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
}

.m26-mode-strip-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.m26-mode-pill-gw {
  font-size: 11px;
  font-weight: 600;
  padding: 2px 8px;
  border-radius: 6px;
  background: #ede9fe;
  color: #7c3aed;
  white-space: nowrap;
}

.m26-mode-strip-desc {
  font-size: 12px;
  color: var(--m26-text-secondary);
}

.m26-mode-segmented {
  display: flex;
  align-items: center;
  gap: 6px;
  background: rgba(0, 0, 0, 0.04);
  padding: 3px;
  border-radius: 8px;
  flex-shrink: 0;
}

.m26-mode-segmented.compact-hermes .m26-mode-btn {
  font-size: 11px;
  padding: 4px 10px;
}

.m26-mode-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border-radius: 6px;
  border: none;
  background: transparent;
  color: var(--m26-text-secondary);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.m26-mode-btn:hover:not(.disabled):not(.active) {
  background: rgba(255, 255, 255, 0.6);
  color: var(--m26-text);
}

.m26-mode-btn.active {
  background: #ffffff;
  color: var(--m26-accent);
  font-weight: 600;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}

.m26-mode-btn.active:hover {
  background: #ffffff;
  color: var(--m26-accent);
}

.m26-mode-btn.disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.m26-mode-subtext {
  font-size: 10px;
  color: var(--m26-text-muted);
}

/* Claude Desktop 专属 MCP 防护横幅 */
.m26-mcp-banner-card {
  padding: 12px 16px;
  border-radius: 12px;
  border: 1px solid rgba(16, 185, 129, 0.25);
  background: rgba(16, 185, 129, 0.04);
  display: flex;
  align-items: center;
  gap: 12px;
}

.m26-mcp-banner-icon {
  color: #059669;
  flex-shrink: 0;
}

.m26-mcp-banner-body {
  flex: 1;
  min-width: 0;
}

.m26-mcp-banner-title {
  font-size: 12.5px;
  font-weight: 650;
  color: #065f46;
}

.m26-mcp-banner-desc {
  font-size: 11.5px;
  color: #047857;
  margin-top: 2px;
  line-height: 1.45;
}

.m26-mcp-banner-desc strong {
  font-weight: 600;
}

.m26-mcp-banner-desc code {
  background: rgba(16, 185, 129, 0.15);
  padding: 1px 4px;
  border-radius: 3px;
  font-family: monospace;
}

.m26-mcp-finder-btn {
  font-size: 11px;
  color: #065f46;
  background: #ffffff;
  border: 1px solid rgba(16, 185, 129, 0.3);
  padding: 4px 10px;
  border-radius: 6px;
  cursor: pointer;
  white-space: nowrap;
  font-weight: 500;
}

.m26-mcp-finder-btn:hover {
  background: #ecfdf5;
}

/* Claude Code 专属终端环境卡片 */
.m26-cli-env-card {
  padding: 12px 16px;
  border-radius: 12px;
  border: 1px solid var(--m26-border);
  background: var(--m26-card-bg);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.m26-cli-env-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-cli-env-title {
  font-size: 12.5px;
  font-weight: 600;
  color: var(--m26-text);
}

.m26-cli-code {
  margin: 0;
  padding: 8px 12px;
  border-radius: 8px;
  background: #0f172a;
  color: #38bdf8;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, monospace;
  font-size: 11px;
  line-height: 1.5;
  overflow-x: auto;
  border: 1px solid #1e293b;
}

.m26-cli-env-tip {
  font-size: 11px;
  color: var(--m26-text-secondary);
}

.m26-cli-env-tip code {
  background: rgba(0, 0, 0, 0.05);
  padding: 1px 4px;
  border-radius: 3px;
}

/* 主配置卡片 */
.m26-client-config-card {
  padding: 14px 18px;
  border-radius: 12px;
  border: 1px solid var(--m26-border);
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
}

.m26-config-head {
  margin-bottom: 12px;
}

.m26-config-title {
  font-size: 13.5px;
  font-weight: 650;
  color: var(--m26-text);
}

.m26-config-subtitle {
  font-size: 11.5px;
  color: var(--m26-text-muted);
  margin-top: 2px;
}

.m26-config-form {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.m26-form-group {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.m26-form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}

@media (max-width: 800px) {
  .m26-form-grid {
    grid-template-columns: 1fr;
  }
}

.m26-form-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--m26-text);
  display: flex;
  align-items: center;
  gap: 6px;
}

.m26-label-hint {
  font-size: 11px;
  color: var(--m26-text-muted);
  font-weight: normal;
}

.m26-field-desc {
  font-size: 11px;
  color: var(--m26-text-muted);
  line-height: 1.4;
}

.m26-select-wrap {
  position: relative;
}

.m26-select,
.m26-input {
  width: 100%;
  height: 32px;
  padding: 0 10px;
  border-radius: 7px;
  border: 1px solid var(--m26-border-bright);
  background: #ffffff;
  color: var(--m26-text);
  font-size: 12px;
  outline: none;
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}

.m26-select:focus,
.m26-input:focus {
  border-color: var(--m26-accent);
  box-shadow: 0 0 0 2px var(--m26-accent-glow);
}

/* 快速推荐模型标签 */
.m26-quick-models {
  display: flex;
  align-items: center;
  gap: 4px;
}

.m26-chip-btn {
  font-size: 10.5px;
  padding: 2px 7px;
  border-radius: 4px;
  border: 1px solid var(--m26-border-bright);
  background: #ffffff;
  color: var(--m26-text-secondary);
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-chip-btn:hover {
  background: #f1f5f9;
  color: var(--m26-accent);
  border-color: var(--m26-accent);
}

.m26-chip-btn.active {
  background: var(--m26-accent, #4f46e5);
  color: #ffffff;
  border-color: var(--m26-accent, #4f46e5);
}

/* 模型统计小药丸 */
.m26-model-count-pill {
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.05);
  color: var(--m26-text-secondary);
  font-weight: 500;
  transition: all 0.2s ease;
}

.m26-model-count-pill.restricted {
  background: rgba(79, 70, 229, 0.1);
  color: #4f46e5;
  font-weight: 600;
}

/* 模型池复选卡片网格 */
.m26-model-chips-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 8px;
  margin-top: 4px;
}

.m26-model-chip-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  border-radius: 8px;
  border: 1px solid var(--m26-border);
  background: #ffffff;
  cursor: pointer;
  user-select: none;
  transition: all 0.15s ease;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.m26-model-chip-card:hover {
  border-color: var(--m26-border-bright);
  background: #f8fafc;
}

.m26-model-chip-card.checked {
  background: rgba(79, 70, 229, 0.03);
  border-color: rgba(79, 70, 229, 0.4);
}

.m26-model-chip-card.primary {
  border-color: var(--m26-accent, #4f46e5);
  box-shadow: 0 0 0 1.5px var(--m26-accent, #4f46e5);
}

.m26-chip-checkbox {
  width: 14px;
  height: 14px;
  border-radius: 3px;
  border: 1px solid #cbd5e1;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  background: #ffffff;
  transition: all 0.15s ease;
}

.m26-chip-checkbox.checked {
  background: var(--m26-accent, #4f46e5);
  border-color: var(--m26-accent, #4f46e5);
  color: #ffffff;
}

.m26-model-vendor-tag {
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 4px;
  font-weight: 600;
  flex-shrink: 0;
  letter-spacing: 0.2px;
}

.m26-model-vendor-tag.claude {
  background: #fff7ed;
  color: #c2410c;
  border: 1px solid #ffedd5;
}

.m26-model-vendor-tag.openai {
  background: #f0fdf4;
  color: #15803d;
  border: 1px solid #dcfce7;
}

.m26-model-vendor-tag.deepseek {
  background: #eff6ff;
  color: #1d4ed8;
  border: 1px solid #dbeafe;
}

.m26-model-vendor-tag.qwen {
  background: #faf5ff;
  color: #7e22ce;
  border: 1px solid #f3e8ff;
}

.m26-model-vendor-tag.glm {
  background: #f0fdfa;
  color: #0f766e;
  border: 1px solid #ccfbf1;
}

.m26-model-vendor-tag.gemini {
  background: #fdf2f8;
  color: #be185d;
  border: 1px solid #fce7f3;
}

.m26-model-vendor-tag.generic {
  background: #f1f5f9;
  color: #475569;
  border: 1px solid #e2e8f0;
}

.m26-model-chip-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11.5px;
  font-weight: 600;
  color: var(--m26-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-primary-star-badge {
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 4px;
  background: #eef2ff;
  color: #4338ca;
  font-weight: 650;
  white-space: nowrap;
}

.m26-set-primary-btn {
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 4px;
  background: transparent;
  border: 1px solid transparent;
  color: var(--m26-text-muted);
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.m26-set-primary-btn:hover {
  background: #f1f5f9;
  color: var(--m26-text);
  border-color: #cbd5e1;
}

.m26-custom-model-input-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

/* 全自动容灾横幅 */
.m26-failover-strip {
  padding: 10px 14px;
  border-radius: 8px;
  border: 1px solid rgba(16, 185, 129, 0.25);
  background: rgba(16, 185, 129, 0.04);
  margin-top: 4px;
}

.m26-failover-icon {
  font-size: 15px;
  margin-top: 1px;
  flex-shrink: 0;
}

.m26-failover-title {
  font-size: 11.5px;
  font-weight: 650;
  color: #065f46;
  margin-bottom: 2px;
}

.m26-failover-desc {
  font-size: 11px;
  color: #047857;
  line-height: 1.45;
}

.m26-failover-desc strong {
  color: #065f46;
}

/* 分发策略三卡片 */
.m26-strategy-cards {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
}

.m26-strategy-card {
  padding: 9px 12px;
  border-radius: 8px;
  border: 1px solid var(--m26-border);
  background: #ffffff;
  cursor: pointer;
  transition: all 0.15s ease;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.m26-strategy-card:hover:not(.selected) {
  background: #f8fafc;
  border-color: var(--m26-border-bright);
}

.m26-strategy-card.selected {
  background: rgba(79, 70, 229, 0.04);
  border-color: var(--m26-accent);
  box-shadow: 0 0 0 1px var(--m26-accent);
}

.m26-strategy-card.selected:hover {
  background: rgba(79, 70, 229, 0.06);
  border-color: var(--m26-accent);
  box-shadow: 0 0 0 1px var(--m26-accent);
}

.m26-strategy-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--m26-text);
}

.m26-strategy-desc {
  font-size: 11px;
  color: var(--m26-text-muted);
  margin-top: 3px;
  line-height: 1.35;
}

/* 渠道复选框网格 */
.m26-checkbox-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 6px;
}

.m26-checkbox-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: 6px;
  border: 1px solid var(--m26-border);
  background: #ffffff;
  cursor: pointer;
  user-select: none;
  transition: all 0.15s ease;
}

.m26-checkbox-item:hover {
  background: #f8fafc;
}

.m26-checkbox-item.checked {
  background: rgba(79, 70, 229, 0.04);
  border-color: var(--m26-accent);
}

.m26-checkbox-box {
  width: 14px;
  height: 14px;
  border-radius: 3px;
  border: 1px solid #cbd5e1;
  display: inline-block;
  position: relative;
  background: #ffffff;
}

.m26-checkbox-item.checked .m26-checkbox-box {
  background: var(--m26-accent);
  border-color: var(--m26-accent);
}

.m26-checkbox-item.checked .m26-checkbox-box::after {
  content: '';
  position: absolute;
  left: 4px;
  top: 1px;
  width: 4px;
  height: 8px;
  border: solid white;
  border-width: 0 1.5px 1.5px 0;
  transform: rotate(45deg);
}

.m26-checkbox-text {
  font-size: 11.5px;
  color: var(--m26-text);
  font-weight: 500;
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-checkbox-proto {
  font-size: 9px;
  color: var(--m26-text-muted);
  background: rgba(0, 0, 0, 0.04);
  padding: 1px 4px;
  border-radius: 3px;
}

/* 连通性测试栏 */
.m26-test-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-top: 2px;
}

.m26-btn-test {
  height: 28px;
  padding: 0 12px;
  border-radius: 6px;
  border: 1px solid var(--m26-border-bright);
  background: #ffffff;
  color: var(--m26-text);
  font-size: 11.5px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.m26-btn-test:hover:not(:disabled) {
  background: #f8fafc;
  border-color: rgba(0, 0, 0, 0.2);
}

.m26-btn-test:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.m26-test-result-msg {
  font-size: 11px;
}

.m26-test-result-msg.testing {
  color: #3b82f6;
}

.m26-test-result-msg.success {
  color: #059669;
}

.m26-test-result-msg.failed {
  color: #e11d48;
}

/* 配置保护与 Diff 预览 */
.m26-config-guard-card {
  padding: 12px 16px;
  border-radius: 12px;
  border: 1px solid rgba(16, 185, 129, 0.25);
  background: rgba(16, 185, 129, 0.03);
}

.m26-guard-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  cursor: pointer;
}

.m26-guard-title {
  font-size: 12px;
  font-weight: 650;
  color: var(--m26-text);
}

.m26-guard-badge {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: rgba(16, 185, 129, 0.12);
  color: #059669;
  font-weight: 500;
}

.m26-guard-toggle {
  font-size: 11px;
  color: var(--m26-text-secondary);
  font-weight: 500;
  cursor: pointer;
}

.m26-guard-toggle:hover {
  color: var(--m26-text);
}

.m26-guard-desc {
  font-size: 11px;
  color: var(--m26-text-secondary);
  margin: 4px 0 0 0;
  line-height: 1.45;
}

.m26-guard-desc code {
  color: var(--m26-text);
  background: rgba(0, 0, 0, 0.05);
  padding: 1px 4px;
  border-radius: 3px;
}

.m26-diff-box {
  margin-top: 10px;
  border-radius: 8px;
  border: 1px solid #334155;
  background: #0f172a;
  overflow: hidden;
}

.m26-diff-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 5px 10px;
  background: #1e293b;
  border-bottom: 1px solid #334155;
}

.m26-diff-status-tag {
  font-size: 10px;
  color: #38bdf8;
  font-weight: 500;
}

.m26-diff-code {
  margin: 0;
  padding: 10px 12px;
  font-family: monospace;
  font-size: 11px;
  color: #e2e8f0;
  line-height: 1.5;
  overflow-x: auto;
}

/* 通用指引卡片 */
.m26-generic-guide-card {
  padding: 14px 18px;
  border-radius: 12px;
  border: 1px solid var(--m26-border);
  background: var(--m26-card-bg);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.02);
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.m26-guide-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-guide-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 12px;
}

@media (max-width: 900px) {
  .m26-guide-grid {
    grid-template-columns: 1fr;
  }
}

.m26-guide-item {
  background: rgba(255, 255, 255, 0.7);
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 10px;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-guide-app-title {
  font-size: 12.5px;
  font-weight: 600;
  color: var(--m26-text, #18181b);
}

.m26-guide-app-desc {
  font-size: 11.5px;
  color: var(--m26-text-secondary, #71717a);
}

.m26-guide-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 11.5px;
}

.m26-guide-list li {
  display: flex;
  align-items: center;
  gap: 6px;
}

.m26-guide-k {
  color: var(--m26-text-secondary, #71717a);
  font-size: 11px;
  min-width: 55px;
}

.m26-guide-list code,
.m26-guide-code code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  padding: 1px 5px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.04);
  color: var(--m26-text, #18181b);
  border: 1px solid rgba(0, 0, 0, 0.06);
}

.m26-guide-code {
  margin: 0;
  padding: 6px 8px;
  border-radius: 6px;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  overflow-x: auto;
}

.m26-guide-code code {
  padding: 0;
  background: transparent;
  border: none;
  font-size: 10.5px;
  line-height: 1.4;
  color: #334155;
}

/* Hermes 专享接入卡片 */
.m26-hermes-access-card {
  padding: 14px 18px;
  border-radius: 12px;
  border: 1px solid rgba(99, 102, 241, 0.2);
  background: linear-gradient(135deg, rgba(238, 242, 255, 0.7) 0%, rgba(245, 243, 255, 0.5) 100%);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 2px 6px rgba(99, 102, 241, 0.04);
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.m26-access-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.m26-access-icon {
  font-size: 18px;
  line-height: 1;
}

.m26-access-title {
  font-size: 13.5px;
  font-weight: 600;
  color: #312e81;
}

.m26-access-subtitle {
  font-size: 11.5px;
  color: #6366f1;
}

.m26-btn-copy-script {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 11px;
  border-radius: 6px;
  border: 1px solid rgba(99, 102, 241, 0.25);
  background: #ffffff;
  color: #4f46e5;
  font-size: 11px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

/* Hermes 整体重新设计：双栏资源池展示齐全 + Auto 与调度模式紧密结合，一屏全览无滚动条 */
.m26-hermes-redesign-container {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  flex: 1;
  gap: 10px;
  overflow: hidden;
}

/* 1. 双栏资源池 (左供应商、右模型) */
.m26-hermes-pool-grid {
  display: grid;
  grid-template-columns: 210px 1fr;
  gap: 10px;
  flex: 1;
  min-height: 0;
  align-items: stretch;
  overflow: hidden;
}

@media (max-width: 900px) {
  .m26-hermes-pool-grid {
    grid-template-columns: 180px 1fr;
  }
}

.m26-pool-panel {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 10px;
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  overflow: hidden;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.03);
}

.m26-panel-header {
  padding: 8px 12px;
  border-bottom: 1px solid #f1f5f9;
  background: #f8fafc;
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex-shrink: 0;
}

.m26-panel-title-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-panel-title {
  font-size: 13px;
  font-weight: 700;
  color: #1e293b;
}

.m26-panel-badge {
  font-size: 11px;
  font-weight: 600;
  color: #4f46e5;
  background: #eef2ff;
  padding: 1px 7px;
  border-radius: 9999px;
  font-family: ui-monospace, Menlo, monospace;
}

.m26-action-link {
  font-size: 11.5px;
  font-weight: 500;
  color: #2563eb;
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: all 0.12s ease;
}

.m26-action-link:hover:not(:disabled) {
  background: #eff6ff;
  text-decoration: underline;
}

.m26-action-link:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* 供应商左侧列表：单行紧凑展示 (一行展示) */
.m26-channel-cards-list {
  padding: 6px;
  display: flex;
  flex-direction: column;
  gap: 3px;
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.m26-channel-select-card {
  display: flex !important;
  flex-direction: row !important;
  align-items: center !important;
  justify-content: space-between !important;
  gap: 8px;
  padding: 5px 8px;
  height: 32px;
  min-height: 32px;
  max-height: 32px;
  border-radius: 6px;
  border: 1px solid transparent;
  cursor: pointer;
  user-select: none;
  transition: all 0.12s ease;
  box-sizing: border-box;
}

.m26-channel-select-card:hover {
  background: #f1f5f9;
}

.m26-channel-select-card.selected {
  background: #f0fdf4 !important;
  border-color: #bbf7d0 !important;
}

.m26-channel-select-card-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
}

.m26-channel-name {
  font-size: 12px;
  font-weight: 500;
  color: #1e293b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: left;
}

.m26-channel-select-card.selected .m26-channel-name {
  color: #15803d;
  font-weight: 600;
}

.m26-channel-count-pill {
  font-size: 10.5px;
  font-weight: 500;
  color: #64748b;
  background: #f1f5f9;
  padding: 1px 5px;
  border-radius: 4px;
  font-family: ui-monospace, Menlo, monospace;
  flex-shrink: 0;
}

.m26-channel-select-card.selected .m26-channel-count-pill {
  background: #dcfce7;
  color: #166534;
}

/* 模型分类单行左右平滑滚动 (鼠标滚轮与手势直接左右滑动) */
.m26-family-tabs-scroll {
  display: flex;
  align-items: center;
  gap: 5px;
  overflow-x: auto;
  overflow-y: hidden;
  flex-wrap: nowrap;
  white-space: nowrap;
  scrollbar-width: none;
  width: 100%;
  min-width: 0;
  scroll-behavior: smooth;
  padding: 1px 0;
  -webkit-overflow-scrolling: touch;
  overscroll-behavior-x: contain;
}

.m26-family-tabs-scroll::-webkit-scrollbar {
  display: none;
}

.m26-family-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2.5px 7px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 500;
  color: #64748b;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.12s ease;
  user-select: none;
  flex-shrink: 0;
}

.m26-family-chip:hover {
  background: #f1f5f9;
  color: #1e293b;
  border-color: #cbd5e1;
}

.m26-family-chip.active {
  background: #4f46e5;
  color: #ffffff;
  border-color: #4f46e5;
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(79, 70, 229, 0.2);
}

.m26-chip-count {
  font-size: 9.5px;
  font-weight: 600;
  padding: 0 4px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.06);
  font-family: ui-monospace, Menlo, monospace;
}

.m26-family-chip.active .m26-chip-count {
  background: rgba(255, 255, 255, 0.25);
  color: #ffffff;
}

.m26-model-cards-scroll {
  padding: 6px 8px;
  display: flex;
  flex-direction: column;
  gap: 3px;
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.m26-model-select-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 5px 8px;
  border-radius: 6px;
  border: 1px solid transparent;
  cursor: pointer;
  user-select: none;
  transition: all 0.12s ease;
}

.m26-model-select-card:hover {
  background: #f1f5f9;
}

.m26-model-select-card.selected {
  background: #eff6ff;
  border-color: #bfdbfe;
}

.m26-model-title {
  font-size: 12.5px;
  font-weight: 600;
  color: #1e293b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-merged-pill {
  font-size: 9.5px;
  font-weight: 600;
  color: #4f46e5;
  background: #e0e7ff;
  padding: 1px 5px;
  border-radius: 3px;
  white-space: nowrap;
}

.m26-model-aliases {
  font-size: 10.5px;
  color: #64748b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-provider-pill {
  font-size: 10.5px;
  color: #64748b;
  background: #f1f5f9;
  padding: 2px 7px;
  border-radius: 4px;
  white-space: nowrap;
}

.m26-model-select-card.selected .m26-provider-pill {
  background: #dbeafe;
  color: #1d4ed8;
}

.m26-empty-state {
  font-size: 12px;
  color: #94a3b8;
  text-align: center;
  padding: 24px 10px;
}

/* 2. 整合控制区 (Auto 智能容灾调度 + 调度模式 + 保存并重启) */
.m26-hermes-top-panel,
.m26-hermes-bottom-panel {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 10px;
  padding: 8px 14px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.03);
  flex-shrink: 0;
  transition: all 0.2s ease;
}

.m26-hermes-top-panel.auto-enabled,
.m26-hermes-bottom-panel.auto-enabled {
  border-color: #c7d2fe;
  background: #fcfdff;
}

.m26-bottom-panel-main {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
}

.m26-panel-left-group {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 1;
  min-width: 0;
}

.m26-panel-auto-title {
  font-size: 13px;
  font-weight: 600;
  color: #1e293b;
  white-space: nowrap;
}

/* Auto 帮助提示按钮与弹出层 */
.m26-auto-help-wrapper {
  position: relative;
  display: inline-flex;
  align-items: center;
}

.m26-hint-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 1px solid #cbd5e1;
  background: #f8fafc;
  color: #64748b;
  cursor: pointer;
  transition: all 0.15s ease;
  padding: 0;
  flex-shrink: 0;
}

.m26-hint-icon-btn:hover,
.m26-hint-icon-btn.active {
  color: #4f46e5;
  border-color: #818cf8;
  background: #eef2ff;
}

.m26-popover-backdrop {
  position: fixed;
  inset: 0;
  z-index: 99;
  background: transparent;
}

.m26-auto-help-popover {
  position: absolute;
  top: calc(100% + 8px);
  left: 0;
  width: 330px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 9px;
  box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.1), 0 8px 10px -6px rgba(0, 0, 0, 0.1);
  padding: 12px 14px;
  z-index: 100;
  line-height: 1.5;
  color: #334155;
  animation: fadeIn 0.15s ease;
}

.m26-help-popover-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
  padding-bottom: 5px;
  border-bottom: 1px solid #f1f5f9;
}

.m26-help-popover-title {
  font-size: 12px;
  font-weight: 700;
  color: #1e293b;
}

.m26-help-popover-close {
  background: none;
  border: none;
  color: #94a3b8;
  cursor: pointer;
  font-size: 11px;
  padding: 0 2px;
  line-height: 1;
}

.m26-help-popover-close:hover {
  color: #1e293b;
}

.m26-help-popover-content {
  font-size: 11.5px;
  color: #475569;
}

.m26-help-block {
  display: flex;
  flex-direction: column;
}

.m26-help-subtitle {
  font-size: 11px;
  font-weight: 700;
  color: #334155;
  margin-bottom: 3px;
}

.m26-help-text {
  font-size: 11.5px;
  color: #475569;
  line-height: 1.5;
  margin: 0;
}

.m26-help-modes-list {
  display: flex;
  flex-direction: column;
  gap: 7px;
  margin-top: 4px;
}

.m26-help-mode-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.m26-mode-badge {
  display: inline-block;
  align-self: flex-start;
  font-size: 10.5px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 4px;
  line-height: 1.4;
}

.m26-mode-badge.quality {
  background: #eff6ff;
  color: #1d4ed8;
  border: 1px solid #bfdbfe;
}

.m26-mode-badge.latency {
  background: #f0fdf4;
  color: #15803d;
  border: 1px solid #bbf7d0;
}

.m26-mode-badge.balance {
  background: #faf5ff;
  color: #7e22ce;
  border: 1px solid #e9d5ff;
}

.m26-mode-desc {
  font-size: 11px;
  color: #64748b;
  line-height: 1.4;
  padding-left: 2px;
}

/* 自定义调度模式下拉选择框 (替换原生 select，彻底解决系统弹窗错位与不对齐) */
.m26-strategy-dropdown-wrap {
  position: relative;
  width: 144px;
  flex-shrink: 0;
  transition: opacity 0.2s ease;
}

.m26-strategy-dropdown-wrap.disabled {
  opacity: 0.45;
  pointer-events: none;
}

.m26-strategy-trigger {
  width: 100%;
  height: 32px;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 7px;
  padding: 0 10px 0 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  cursor: pointer;
  user-select: none;
  transition: all 0.15s ease;
  outline: none;
}

.m26-strategy-trigger:hover:not(:disabled) {
  border-color: #94a3b8;
  background: #f8fafc;
}

.m26-strategy-trigger.open {
  border-color: #4f46e5;
  box-shadow: 0 0 0 2px rgba(79, 70, 229, 0.12);
  background: #ffffff;
}

.m26-strategy-trigger:disabled {
  background: #f1f5f9;
  cursor: not-allowed;
  border-color: #e2e8f0;
}

.m26-strategy-trigger-text {
  font-size: 12px;
  font-weight: 500;
  color: #1e293b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  text-align: left;
}

.m26-strategy-trigger-arrow {
  width: 15px;
  height: 15px;
  color: #64748b;
  flex-shrink: 0;
  transition: transform 0.2s ease, color 0.2s ease;
}

.m26-strategy-trigger-arrow.rotated {
  transform: rotate(180deg);
  color: #4f46e5;
}

.m26-strategy-options-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  width: 100%;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 8px;
  box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.1), 0 8px 10px -6px rgba(0, 0, 0, 0.06);
  padding: 4px;
  z-index: 100;
  display: flex;
  flex-direction: column;
  gap: 2px;
  animation: fadeIn 0.12s ease;
}

.m26-strategy-option-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 7px 10px;
  border-radius: 6px;
  font-size: 12px;
  cursor: pointer;
  color: #334155;
  transition: all 0.12s ease;
  user-select: none;
}

.m26-strategy-option-item:hover {
  background: #f1f5f9;
  color: #0f172a;
}

.m26-strategy-option-item.selected {
  background: #eff6ff;
  color: #1d4ed8;
  font-weight: 600;
}

.m26-option-check-icon {
  width: 14px;
  height: 14px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: #2563eb;
}

.m26-option-text-main {
  flex-shrink: 0;
}

.m26-option-text-desc {
  font-size: 11px;
  color: #64748b;
}

.m26-strategy-option-item.selected .m26-option-text-desc {
  color: #3b82f6;
}

.m26-panel-right-group {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.m26-btn-save-restart {
  height: 32px;
  padding: 0 16px;
  background: #2563eb;
  color: #ffffff;
  border: none;
  border-radius: 7px;
  font-size: 12.5px;
  font-weight: 500;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  white-space: nowrap;
  box-shadow: 0 1px 2px rgba(37, 99, 235, 0.2);
  transition: all 0.15s ease;
}

.m26-btn-save-restart:hover:not(:disabled) {
  background: #1d4ed8;
  box-shadow: 0 2px 5px rgba(37, 99, 235, 0.3);
  transform: translateY(-0.5px);
}

.m26-btn-save-restart:active:not(:disabled) {
  transform: translateY(0);
}

.m26-btn-save-restart:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.m26-panel-feedback-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  border-radius: 6px;
  font-size: 12px;
  animation: fadeIn 0.15s ease;
}

.m26-panel-feedback-bar.success {
  background: #f0fdf4;
  border: 1px solid #bbf7d0;
  color: #15803d;
}

.m26-panel-feedback-bar.error {
  background: #fef2f2;
  border: 1px solid #fecaca;
  color: #b91c1c;
}

.m26-feedback-close {
  background: none;
  border: none;
  cursor: pointer;
  color: inherit;
  opacity: 0.6;
  font-size: 12px;
  padding: 0 4px;
}

.m26-feedback-close:hover {
  opacity: 1;
}


@media (max-width: 860px) {
  .m26-bottom-panel-main {
    flex-wrap: wrap;
    gap: 10px;
  }
  .m26-panel-left-group {
    flex-wrap: wrap;
  }
}

.m26-step-header {
  display: flex;
  align-items: flex-start;
  gap: 12px;
}

.m26-step-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 2px 8px;
  border-radius: 6px;
  background: #6366f1;
  color: #ffffff;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.02em;
  flex-shrink: 0;
  margin-top: 1px;
}

.m26-step-titles {
  flex: 1;
  min-width: 0;
}

.m26-step-title {
  font-size: 13.5px;
  font-weight: 600;
  color: var(--m26-text, #18181b);
  display: flex;
  align-items: center;
  gap: 8px;
}

.m26-step-subtitle {
  font-size: 11.5px;
  color: var(--m26-text-secondary, #71717a);
  margin-top: 2px;
}

.m26-channel-scope-tag {
  font-size: 11px;
  font-weight: 500;
  color: #4f46e5;
  background: #eef2ff;
  border: 1px solid rgba(99, 102, 241, 0.2);
  padding: 1px 7px;
  border-radius: 4px;
}

.m26-step-quick-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.m26-mini-btn {
  padding: 3px 8px;
  font-size: 11px;
  font-weight: 500;
  color: #4b5563;
  background: #ffffff;
  border: 1px solid #d1d5db;
  border-radius: 5px;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-mini-btn:hover {
  background: #f3f4f6;
  color: #111827;
  border-color: #9ca3af;
}

.m26-check-btn {
  display: inline-flex;
  align-items: center;
  font-size: 11px;
  color: #4338ca;
  background: #eef2ff;
  border-color: #c7d2fe;
}

.m26-check-btn:hover:not(:disabled) {
  background: #e0e7ff;
  border-color: #a5b4fc;
}

.m26-check-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* 防降智与质量保障提示条 */
.m26-quality-guard-banner {
  display: flex;
  align-items: center;
  gap: 8px;
  background: #f0fdf4;
  border: 1px solid #bbf7d0;
  border-radius: 7px;
  padding: 6px 10px;
  margin-bottom: 12px;
  font-size: 11.5px;
  color: #166534;
  line-height: 1.4;
}

.m26-quality-guard-icon {
  font-size: 14px;
  flex-shrink: 0;
}

.m26-guard-highlight {
  font-weight: 700;
  color: #15803d;
}

/* 渠道质量徽章 */
.m26-quality-badge {
  font-size: 10px;
  font-weight: 600;
  padding: 1px 5px;
  border-radius: 4px;
  white-space: nowrap;
}

.m26-quality-badge.clean {
  background: #ecfdf5;
  color: #059669;
  border: 1px solid #a7f3d0;
}

.m26-quality-badge.suspected {
  background: #fffbeb;
  color: #d97706;
  border: 1px solid #fde68a;
}

.m26-quality-badge.detected {
  background: #fef2f2;
  color: #dc2626;
  border: 1px solid #fecaca;
}

.m26-quality-badge.unknown {
  background: #f3f4f6;
  color: #6b7280;
  border: 1px solid #e5e7eb;
}



/* 步骤 2: 当前主脑指示条 */
.m26-brain-summary-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: rgba(243, 244, 246, 0.8);
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 8px;
  padding: 8px 12px;
}

.m26-brain-summary-bar.auto_mode {
  background: #f0fdf4;
  border-color: #bbf7d0;
}

.m26-brain-highlight {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  font-weight: 700;
  color: #1e1b4b;
  background: #e0e7ff;
  border: 1px solid #c7d2fe;
  padding: 1px 6px;
  border-radius: 4px;
}

.m26-brain-highlight.is_auto {
  background: #dcfce7;
  color: #15803d;
  border-color: #86efac;
}

/* ✨ Auto 智能调度面板与开关 */
.m26-auto-control-panel {
  background: #ffffff;
  border: 1.5px solid #e2e8f0;
  border-radius: 9px;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  transition: all 0.18s ease;
}

.m26-auto-control-panel.active {
  background: #f8fafc;
  border-color: #6366f1;
  box-shadow: 0 0 0 1px #6366f1, 0 3px 8px rgba(99, 102, 241, 0.08);
}

.m26-auto-control-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.m26-auto-icon-bubble {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: #eef2ff;
  border: 1px solid #c7d2fe;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 15px;
  flex-shrink: 0;
}

.m26-auto-main-label {
  font-size: 13.5px;
  font-weight: 700;
  color: #0f172a;
}

.m26-auto-status-tag {
  font-size: 10.5px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 4px;
}

.m26-auto-status-tag.enabled {
  background: #ecfdf5;
  color: #059669;
  border: 1px solid #a7f3d0;
}

.m26-auto-status-tag.disabled {
  background: #f1f5f9;
  color: #64748b;
  border: 1px solid #e2e8f0;
}

.m26-auto-sub-desc {
  font-size: 11.5px;
  color: #64748b;
  line-height: 1.35;
  margin-top: 2px;
}

/* 现代化开关滑动条 */
.m26-switch-label {
  position: relative;
  display: inline-block;
  width: 42px;
  height: 24px;
  cursor: pointer;
  flex-shrink: 0;
}

.m26-switch-label input {
  opacity: 0;
  width: 0;
  height: 0;
}

.m26-switch-track {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background-color: #cbd5e1;
  border-radius: 24px;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.m26-switch-track:before {
  position: absolute;
  content: "";
  height: 18px;
  width: 18px;
  left: 3px;
  bottom: 3px;
  background-color: white;
  border-radius: 50%;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.m26-switch-label input:checked + .m26-switch-track {
  background-color: #4f46e5;
}

.m26-switch-label input:checked + .m26-switch-track:before {
  transform: translateX(18px);
}

/* Auto 调度策略选择框 */
.m26-auto-strategy-selector {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-top: 8px;
  border-top: 1px dashed #e2e8f0;
}

.m26-auto-strategy-heading {
  font-size: 11.5px;
  font-weight: 600;
  color: #475569;
  flex-shrink: 0;
}

.m26-auto-strategy-pills {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.m26-auto-strategy-pill {
  padding: 3px 9px;
  font-size: 11px;
  font-weight: 500;
  border-radius: 6px;
  border: 1px solid #cbd5e1;
  background: #ffffff;
  color: #334155;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-auto-strategy-pill:hover {
  border-color: #94a3b8;
  background: #f8fafc;
}

.m26-auto-strategy-pill.selected {
  border-color: #4f46e5;
  background: #eef2ff;
  color: #4338ca;
  font-weight: 600;
}

.m26-hermes-model-card.in_auto_pool {
  border-color: #818cf8;
  background: #faf5ff;
}

.m26-auto-in-pool-pill {
  font-size: 10px;
  font-weight: 600;
  color: #6366f1;
  background: #eef2ff;
  padding: 1px 5px;
  border-radius: 3px;
}

/* 步骤 2: 模型网格 */
.m26-hermes-models-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 8px;
}

.m26-hermes-model-card {
  background: #ffffff;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  padding: 9px 12px;
  cursor: pointer;
  transition: all 0.12s ease;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  user-select: none;
}

.m26-hermes-model-card:hover {
  border-color: #cbd5e1;
  background: #f8fafc;
}

.m26-hermes-model-card.is_primary {
  border-color: #f59e0b;
  background: #fffbeb;
  box-shadow: 0 0 0 1px rgba(245, 158, 11, 0.4);
}

.m26-hermes-model-card.is_allowed:not(.is_primary) {
  border-color: #c7d2fe;
}

.m26-model-checkbox {
  width: 17px;
  height: 17px;
  border-radius: 4px;
  border: 1.5px solid #d1d5db;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  font-weight: 700;
  color: #ffffff;
  cursor: pointer;
  flex-shrink: 0;
  transition: all 0.12s ease;
}

.m26-model-checkbox.checked {
  background: #4f46e5;
  border-color: #4f46e5;
}

.m26-model-texts {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.m26-model-name-text {
  font-size: 12px;
  font-weight: 600;
  color: #1f2937;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-model-status-sub {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 10px;
}

.m26-primary-badge {
  color: #b45309;
  font-weight: 700;
}

.m26-click-set-hint {
  color: #9ca3af;
}

.m26-hermes-model-card:hover .m26-click-set-hint {
  color: #6366f1;
}

.m26-allowed-pill {
  color: #15803d;
  background: #dcfce7;
  padding: 0 4px;
  border-radius: 3px;
  font-size: 9.5px;
  font-weight: 500;
}

.m26-set-brain-btn {
  padding: 2px 7px;
  font-size: 10.5px;
  font-weight: 500;
  color: #4b5563;
  background: #f3f4f6;
  border: 1px solid #d1d5db;
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-set-brain-btn:hover {
  background: #eef2ff;
  color: #4338ca;
  border-color: #a5b4fc;
}

.m26-star-icon {
  color: #d97706;
  font-size: 14px;
}

.m26-counter-badge {
  font-size: 11px;
  font-weight: 500;
  color: #4f46e5;
  background: #eef2ff;
  border: 1px solid rgba(99, 102, 241, 0.2);
  padding: 1px 7px;
  border-radius: 4px;
}

.m26-mini-checkbox {
  width: 15px;
  height: 15px;
  border-radius: 3.5px;
  border: 1.5px solid #d1d5db;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 700;
  color: #ffffff;
  flex-shrink: 0;
  transition: all 0.12s ease;
}

.m26-mini-checkbox.checked,
.m26-mini-checkbox.indeterminate {
  background: #2563eb;
  border-color: #2563eb;
}

.m26-model-provider-tags {
  font-size: 10px;
  color: #6b7280;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-dropdown-combobox-wrapper {
  position: relative;
  width: 100%;
  z-index: 20;
}

.m26-dropdown-combobox-wrapper.has-open-dropdown {
  z-index: 100;
}

.m26-combobox-input {
  min-height: 42px;
  background: #ffffff;
  border: 1.5px solid #e2e8f0;
  border-radius: 9px;
  padding: 6px 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  cursor: pointer;
  transition: all 0.15s ease;
  user-select: none;
}

.m26-combobox-input:hover:not(.disabled) {
  border-color: #94a3b8;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.04);
}

.m26-combobox-input.is_open {
  border-color: #4f46e5;
  box-shadow: 0 0 0 2px rgba(79, 70, 229, 0.15);
}

.m26-combobox-input.disabled {
  background: #f8fafc;
  cursor: not-allowed;
  opacity: 0.7;
}

.m26-combobox-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
  flex-wrap: wrap;
}

.m26-combobox-icon {
  font-size: 15px;
  flex-shrink: 0;
}

.m26-combobox-placeholder {
  font-size: 12.5px;
  color: #94a3b8;
}

.m26-combobox-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  max-height: 88px;
  overflow-y: auto;
  padding: 2px 0;
}

.m26-tag-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  background: #eef2ff;
  border: 1px solid #c7d2fe;
  border-radius: 5px;
  font-size: 11.5px;
  font-weight: 500;
  color: #3730a3;
  line-height: 1.3;
}

.m26-tag-pill.model {
  background: #f5f3ff;
  border-color: #ddd6fe;
  color: #5b21b6;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}

.m26-tag-remove {
  border: none;
  background: transparent;
  color: #6366f1;
  font-size: 13px;
  line-height: 1;
  cursor: pointer;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  width: 14px;
  height: 14px;
  transition: all 0.1s ease;
}

.m26-tag-remove:hover {
  background: rgba(99, 102, 241, 0.2);
  color: #312e81;
}

.m26-combobox-right {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.m26-combobox-clear-btn {
  border: none;
  background: #f1f5f9;
  color: #64748b;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  font-size: 10px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.1s ease;
}

.m26-combobox-clear-btn:hover {
  background: #e2e8f0;
  color: #0f172a;
}

.m26-combobox-arrow {
  font-size: 12px;
  color: #64748b;
  transition: transform 0.2s ease;
}

.m26-combobox-arrow.rotated {
  transform: rotate(180deg);
}

/* 下拉菜单浮层 */
.m26-dropdown-menu {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  right: 0;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 10px;
  box-shadow: 0 16px 36px -4px rgba(0, 0, 0, 0.18), 0 8px 16px -6px rgba(0, 0, 0, 0.08);
  z-index: 999;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  animation: m26-dropdown-appear 0.15s ease-out;
}

@keyframes m26-dropdown-appear {
  from {
    opacity: 0;
    transform: translateY(-4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.m26-dropdown-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 8px;
  border-bottom: 1px solid #f1f5f9;
}

.m26-dropdown-select-all {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 8px;
  border-radius: 6px;
  cursor: pointer;
  user-select: none;
  flex-shrink: 0;
  transition: background-color 0.12s ease;
}

.m26-dropdown-select-all:hover {
  background-color: #f1f5f9;
}

.m26-select-all-label {
  font-size: 12px;
  font-weight: 600;
  color: #334155;
  white-space: nowrap;
}

.m26-indeterminate-mark {
  line-height: 1;
  font-size: 13px;
  font-weight: 800;
  display: block;
  transform: translateY(-1px);
}

.m26-dropdown-search-box {
  position: relative;
  flex: 1;
  display: flex;
  align-items: center;
}

.m26-search-icon {
  position: absolute;
  left: 9px;
  font-size: 12px;
  color: #94a3b8;
  pointer-events: none;
}

.m26-dropdown-search-input {
  width: 100%;
  height: 30px;
  padding: 0 26px 0 28px;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  font-size: 12px;
  outline: none;
  transition: all 0.12s ease;
}

.m26-dropdown-search-input:focus {
  border-color: #2563eb;
  box-shadow: 0 0 0 2px rgba(37, 99, 235, 0.1);
}

.m26-search-clear {
  position: absolute;
  right: 7px;
  border: none;
  background: transparent;
  color: #94a3b8;
  font-size: 11px;
  cursor: pointer;
}

.m26-dropdown-list {
  max-height: 240px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-right: 2px;
}

.m26-dropdown-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 7px 10px;
  border-radius: 6px;
  cursor: pointer;
  transition: background-color 0.1s ease;
  user-select: none;
}

.m26-dropdown-item:hover {
  background-color: #f8fafc;
}

.m26-dropdown-item.selected {
  background-color: #eef2ff;
}

.m26-dropdown-item-name {
  font-size: 12.5px;
  font-weight: 500;
  color: #1e293b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-merged-badge {
  font-size: 10px;
  font-weight: 600;
  color: #4f46e5;
  background: #e0e7ff;
  padding: 1px 5px;
  border-radius: 3px;
  white-space: nowrap;
}

.m26-model-alias-sub {
  font-size: 10.5px;
  color: #64748b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-dropdown-empty {
  font-size: 12px;
  color: #94a3b8;
  text-align: center;
  padding: 20px 10px;
}

/* 图二模型分类 Tabs 与分组样式 */
.m26-model-dropdown-menu {
  max-height: 420px;
}

.m26-dropdown-family-tabs {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 8px 0 6px;
  border-bottom: 1px solid #f1f5f9;
  overflow-x: auto;
  scrollbar-width: none;
}

.m26-dropdown-family-tabs::-webkit-scrollbar {
  display: none;
}

.m26-dropdown-family-tab {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border-radius: 9999px;
  font-size: 11px;
  font-weight: 500;
  color: #64748b;
  background: #f1f5f9;
  border: 1px solid transparent;
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.12s ease;
  flex-shrink: 0;
}

.m26-dropdown-family-tab:hover {
  background: #e2e8f0;
  color: #334155;
}

.m26-dropdown-family-tab.active {
  background: #2563eb;
  color: #ffffff;
  border-color: #2563eb;
  font-weight: 600;
}

.m26-tab-count {
  font-size: 10px;
  padding: 0 4px;
  border-radius: 9999px;
  background: rgba(0, 0, 0, 0.06);
}

.m26-dropdown-family-tab.active .m26-tab-count {
  background: rgba(255, 255, 255, 0.25);
  color: #ffffff;
}

.m26-model-dropdown-list {
  max-height: 290px;
}

.m26-dropdown-group {
  margin-bottom: 6px;
}

.m26-dropdown-group:last-child {
  margin-bottom: 0;
}

.m26-group-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 10px 4px;
  background: #f8fafc;
  border-radius: 5px;
  margin: 4px 0 3px;
  user-select: none;
}

.m26-group-title {
  font-size: 11.5px;
  font-weight: 700;
  color: #334155;
}

.m26-group-count {
  font-size: 10.5px;
  color: #94a3b8;
  font-family: ui-monospace, SFMono-Regular, monospace;
}

.m26-group-action-btn {
  font-size: 11px;
  color: #2563eb;
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 1px 4px;
  border-radius: 3px;
  transition: background-color 0.1s ease;
}

.m26-group-action-btn:hover {
  background-color: #eff6ff;
  text-decoration: underline;
}

/* 步骤 3: 调度策略 4 列网格 */
.m26-strategy-row-4 {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 10px;
}

@media (max-width: 1100px) {
  .m26-strategy-row-4 {
    grid-template-columns: repeat(2, 1fr);
  }
}

@media (max-width: 640px) {
  .m26-strategy-row-4 {
    grid-template-columns: 1fr;
  }
}

/* 手动优先级排序调整面板 */
.m26-manual-sort-section {
  background: #f8fafc;
  border: 1.5px dashed #cbd5e1;
  border-radius: 10px;
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.m26-manual-sort-header {
  display: flex;
  align-items: flex-start;
  gap: 10px;
}

.m26-sort-header-icon {
  font-size: 18px;
  color: #4f46e5;
  flex-shrink: 0;
  line-height: 1.2;
}

.m26-sort-header-title {
  font-size: 12.5px;
  font-weight: 700;
  color: #1e293b;
}

.m26-sort-header-subtitle {
  font-size: 11px;
  color: #64748b;
  margin-top: 2px;
}

.m26-manual-sort-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.m26-manual-sort-item {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 7px;
  padding: 8px 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  transition: all 0.12s ease;
}

.m26-manual-sort-item.is_first {
  border-color: #f59e0b;
  background: #fffdf5;
  box-shadow: 0 1px 3px rgba(245, 158, 11, 0.12);
}

.m26-order-rank {
  font-size: 10.5px;
  font-weight: 700;
  padding: 2px 7px;
  border-radius: 4px;
  background: #f1f5f9;
  color: #475569;
  font-family: ui-monospace, Menlo, monospace;
  white-space: nowrap;
}

.m26-order-rank.rank_one {
  background: #fef3c7;
  color: #b45309;
  border: 1px solid #fde68a;
}

.m26-order-model-name {
  font-size: 12.5px;
  font-weight: 600;
  color: #0f172a;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-order-btn {
  padding: 3px 8px;
  font-size: 11px;
  font-weight: 500;
  color: #475569;
  background: #f8fafc;
  border: 1px solid #cbd5e1;
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.1s ease;
}

.m26-order-btn:hover:not(:disabled) {
  background: #eef2ff;
  color: #4338ca;
  border-color: #a5b4fc;
}

.m26-order-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.m26-empty-sort-hint,
.m26-empty-scope-hint {
  font-size: 12px;
  color: #64748b;
  background: #ffffff;
  border: 1px dashed #cbd5e1;
  border-radius: 8px;
  padding: 16px;
  text-align: center;
}

/* 步骤 4: 底部一键生效卡片 */
.m26-hermes-apply-section {
  background: linear-gradient(135deg, rgba(238, 242, 255, 0.95) 0%, rgba(245, 243, 255, 0.85) 100%);
  border: 1.5px solid rgba(99, 102, 241, 0.25);
  border-radius: 14px;
  padding: 18px 20px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  box-shadow: 0 4px 12px rgba(99, 102, 241, 0.08);
}

.m26-apply-section-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.m26-apply-bolt {
  font-size: 22px;
  line-height: 1;
}

.m26-apply-main-title {
  font-size: 14.5px;
  font-weight: 700;
  color: #312e81;
}

.m26-apply-main-desc {
  font-size: 12px;
  color: #4338ca;
  margin-top: 2px;
}

.m26-apply-main-desc code {
  font-family: ui-monospace, Menlo, monospace;
  font-weight: 600;
  background: rgba(255, 255, 255, 0.8);
  padding: 1px 5px;
  border-radius: 4px;
  color: #1e1b4b;
}

.m26-apply-feedback-alert {
  padding: 9px 14px;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 600;
  display: flex;
  align-items: center;
  animation: fadeIn 0.15s ease-in-out;
}

.m26-apply-feedback-alert.success {
  background: #ecfdf5;
  border: 1px solid #6ee7b7;
  color: #065f46;
}

.m26-apply-feedback-alert.error {
  background: #fef2f2;
  border: 1px solid #fca5a5;
  color: #991b1b;
}

.m26-apply-action-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.m26-btn-hero-apply {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 9px 18px;
  font-size: 13px;
  font-weight: 600;
  color: #ffffff;
  background: linear-gradient(135deg, #4f46e5 0%, #4338ca 100%);
  border: 1px solid rgba(67, 56, 202, 0.3);
  border-radius: 8px;
  cursor: pointer;
  box-shadow: 0 2px 5px rgba(79, 70, 229, 0.25);
  transition: all 0.15s ease;
}

.m26-btn-hero-apply:hover:not(:disabled) {
  background: linear-gradient(135deg, #4338ca 0%, #3730a3 100%);
  transform: translateY(-1px);
  box-shadow: 0 4px 10px rgba(79, 70, 229, 0.35);
}

.m26-btn-hero-apply:disabled {
  opacity: 0.65;
  cursor: not-allowed;
}

.m26-btn-hero-terminal {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 9px 16px;
  font-size: 12.5px;
  font-weight: 600;
  color: #1f2937;
  background: #ffffff;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-btn-hero-terminal:hover:not(:disabled) {
  background: #f9fafb;
  border-color: #9ca3af;
  color: #111827;
}

.m26-btn-hero-ghost {
  display: inline-flex;
  align-items: center;
  padding: 9px 14px;
  font-size: 12px;
  font-weight: 500;
  color: #6b7280;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.m26-btn-hero-ghost:hover:not(:disabled) {
  color: #374151;
  background: rgba(255, 255, 255, 0.5);
}

.m26-channel-pool-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.m26-pool-action-btn {
  padding: 2px 8px;
  font-size: 11px;
  color: var(--m26-text-secondary, #64748b);
  background: #ffffff;
  border: 1px solid var(--m26-border, #e2e8f0);
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.12s ease;
}

.m26-pool-action-btn:hover {
  background: #f8fafc;
  color: var(--m26-text, #1e293b);
  border-color: #cbd5e1;
}

.m26-checkbox-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
  margin-right: 2px;
}

.m26-checkbox-status-dot.healthy {
  background: #10b981;
}

.m26-checkbox-status-dot.degraded {
  background: #f59e0b;
}

.m26-checkbox-status-dot.offline {
  background: #ef4444;
}

.m26-checkbox-status-dot.unknown {
  background: #94a3b8;
}
</style>
