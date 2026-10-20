<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { Channel } from '@/components/channels/types'
import {
  getClientConfigs,
  getGatewayConnection,
  updateClientConfig,
  type ClientPlatformBackendConfig,
} from '@/services/gatewayApi'
import type { GatewayConnectionInfo } from '@/services/gatewayApi'
import { type ClientPlatformId, type ClientPlatformItem, type ClientGatewayStrategy } from './types'

const props = defineProps<{ channels: Channel[] }>()

const platforms = ref<Record<ClientPlatformId, ClientPlatformItem>>({
  'hermes': {
    id: 'hermes',
    name: 'Hermes',
    gateway: {
      strategy: 'smart_quality',
      allowedChannels: [],
      model: 'claude-3-7-sonnet-latest',
      fallbackChannel: '',
      allowedModels: [],
    },
  }
})

// 本地草稿恢复
const STORAGE_KEY = 'heigate_client_platforms_draft'
function persistClientDraft() {
  if (typeof localStorage === 'undefined') return
  try {
    // Channel keys are already available from the in-memory channel list.
    // Never persist them with client preferences in browser storage.
    localStorage.setItem(STORAGE_KEY, JSON.stringify(platforms.value, (key, value) => {
      return key === 'apiKey' ? undefined : value
    }))
  } catch {
    // Ignore storage errors
  }
}

if (typeof localStorage !== 'undefined') {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      const parsed = JSON.parse(raw)
      const savedHermes = parsed?.hermes
      if (savedHermes && typeof savedHermes === 'object') {
        const legacySmallModel = savedHermes.smallModel ?? savedHermes.direct?.smallModel
        if (typeof legacySmallModel === 'string') {
          platforms.value.hermes.smallModel = legacySmallModel
        }
        if (savedHermes.gateway && typeof savedHermes.gateway === 'object') {
          platforms.value.hermes.gateway = { ...platforms.value.hermes.gateway, ...savedHermes.gateway }
        }
      }
    }
  } catch {
    // 忽略解析错误
  } finally {
    // Remove secrets written by previous versions as soon as this view loads.
    persistClientDraft()
  }
}

// 自动持久化草稿
watch(
  platforms,
  persistClientDraft,
  { deep: true }
)

onMounted(async () => {
  if (typeof localStorage !== 'undefined') {
    try {
      for (const key of [
        'heigate_codex_access_profiles_v1',
        'heigate_codex_active_profile_v1',
        'heigate_codex_selected_profile_v1',
      ]) {
        localStorage.removeItem(key)
      }
    } catch {
      // Profile cleanup is best-effort; Hermes settings can still load.
    }
  }
  try {
    const [configs, connection] = await Promise.all([getClientConfigs(), getGatewayConnection()])
    gatewayConnection.value = connection
    const config = configs.hermes
    if (config) {
      if (config.strategy) platforms.value.hermes.gateway.strategy = config.strategy as ClientGatewayStrategy
      if (config.allowed_channels) platforms.value.hermes.gateway.allowedChannels = config.allowed_channels
      if (config.model) platforms.value.hermes.gateway.model = config.model
      if (config.allowed_models) platforms.value.hermes.gateway.allowedModels = config.allowed_models
      isHermesAutoEnabled.value = Boolean(config.auto_enabled ?? config.model === 'Auto')
      hermesStrategy.value = (config.auto_strategy || config.strategy || 'smart_quality') as ClientGatewayStrategy
      if (config.allowed_channels?.length) hermesSelectedChannels.value = [...config.allowed_channels]
      if (config.allowed_models?.length) platforms.value.hermes.gateway.allowedModels = [...config.allowed_models]
    }
  } catch {
    // Keep the local Hermes defaults when the management API is unavailable.
  } finally {
    gatewayConfigLoaded.value = true
    gatewayConfigSaveState.value = 'saved'
    savedGatewayConfigSignature.value = JSON.stringify(sharedGatewayConfig.value)
  }
})
// Hermes 专享：供应商多选、模型多选与调度状态
const hermesSelectedChannels = ref<string[]>([])
const isHermesAutoEnabled = ref(true)
const showHermesAutoHelp = ref(false)
const hermesStrategy = ref<ClientGatewayStrategy>('smart_quality')
const isHermesStrategyDropdownOpen = ref(false)
const gatewayConnection = ref<GatewayConnectionInfo | null>(null)
const gatewayConfigLoaded = ref(false)
const gatewayConfigSaveState = ref<'loading' | 'saving' | 'saved' | 'error'>('loading')
const savedGatewayConfigSignature = ref('')
const copiedGatewayField = ref<'address' | 'key' | null>(null)
let gatewayConfigSaveTimer: ReturnType<typeof setTimeout> | undefined
let pendingGatewayConfig: ClientPlatformBackendConfig | null = null
let gatewayConfigSaveInFlight = false

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

// List exact model IDs from the selected channels. Only case variants of the
// same ID are deduplicated; aliases and model families are never merged.
interface HermesModelItem {
  name: string
  providers: string[]
  familyKey: string
  familyLabel: string
}

const selectedHermesModelFamily = ref<string>('all')

const hermesAvailableModels = computed<HermesModelItem[]>(() => {
  const groupMap = new Map<string, {
    name: string
    providers: Set<string>
  }>()

  for (const ch of hermesAvailableChannels.value) {
    if (!hermesSelectedChannels.value.includes(ch.name)) continue
    const list = (ch.modelNames && ch.modelNames.length > 0)
      ? ch.modelNames
      : (ch.primaryModel ? [ch.primaryModel] : [])
    for (const raw of list) {
      const trimmed = (raw || '').trim()
      if (!trimmed) continue
      const groupKey = trimmed.toLowerCase()
      const current = groupMap.get(groupKey) || {
        name: trimmed,
        providers: new Set<string>(),
      }
      current.providers.add(ch.name)
      groupMap.set(groupKey, current)
    }
  }

  const result: HermesModelItem[] = []
  groupMap.forEach((data) => {
    const family = getModelFamily(data.name)
    result.push({
      name: data.name,
      providers: Array.from(data.providers),
      familyKey: family.key,
      familyLabel: family.label,
    })
  })

  return result.sort((a, b) => a.name.localeCompare(b.name))
})

// 图二模型分类 Tabs
const hermesModelFamilyTabs = computed(() => {
  const counts: Record<string, number> = {}
  hermesAvailableModels.value.forEach((m) => {
    const fam = m.familyKey
    counts[fam] = (counts[fam] || 0) + 1
  })
  const tabs = [{ key: 'all', label: '全部', count: hermesAvailableModels.value.length }]
  MODEL_FAMILY_ORDER.forEach((k) => {
    if (counts[k]) {
      tabs.push({ key: k, label: MODEL_FAMILY_LABELS[k] || k, count: counts[k] })
    }
  })
  return tabs
})

const allHermesAvailableModelNames = computed<string[]>(() => {
  return hermesAvailableModels.value.map((m) => m.name)
})

// Limit the selectable IDs to models exposed by the selected channels.
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
    // If saved choices no longer match the selected channels, select their current IDs.
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
  let list = hermesAvailableModels.value
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
    const familyModels = hermesAvailableModels.value
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
      hermesAvailableModels.value
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
  const familyModels = hermesAvailableModels.value
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


const sharedGatewayConfig = computed<ClientPlatformBackendConfig>(() => {
  const platform = platforms.value.hermes
  const allowedModels = effectiveHermesAllowedModels.value.length > 0
    ? [...effectiveHermesAllowedModels.value]
    : [...allHermesAvailableModelNames.value]

  return {
    strategy: hermesStrategy.value,
    allowed_channels: [...hermesSelectedChannels.value],
    model: isHermesAutoEnabled.value
      ? 'Auto'
      : (allowedModels[0] || platform.gateway.model || 'Auto'),
    allowed_models: allowedModels,
    auto_enabled: isHermesAutoEnabled.value,
    auto_strategy: hermesStrategy.value,
    auto_models: allowedModels,
    apply_local: false,
    restart: false,
    launch_terminal: false,
  }
})

async function flushGatewayConfig() {
  if (gatewayConfigSaveInFlight || !pendingGatewayConfig) return
  const payload = pendingGatewayConfig
  pendingGatewayConfig = null
  gatewayConfigSaveInFlight = true
  const result = await updateClientConfig('hermes', payload)
  gatewayConfigSaveInFlight = false

  if (pendingGatewayConfig) {
    gatewayConfigSaveState.value = 'saving'
    if (!gatewayConfigSaveTimer) void flushGatewayConfig()
    return
  }

  if (result.ok) {
    savedGatewayConfigSignature.value = JSON.stringify(payload)
    gatewayConfigSaveState.value = 'saved'
  } else {
    gatewayConfigSaveState.value = 'error'
  }
}

watch(sharedGatewayConfig, (payload) => {
  if (!gatewayConfigLoaded.value) return
  if (JSON.stringify(payload) === savedGatewayConfigSignature.value) return

  pendingGatewayConfig = payload
  gatewayConfigSaveState.value = 'saving'
  if (gatewayConfigSaveTimer) clearTimeout(gatewayConfigSaveTimer)
  gatewayConfigSaveTimer = setTimeout(() => {
    gatewayConfigSaveTimer = undefined
    void flushGatewayConfig()
  }, 350)
}, { deep: true })

async function copyGatewayField(field: 'address' | 'key') {
  const value = field === 'address'
    ? gatewayConnection.value?.base_url
    : gatewayConnection.value?.api_key
  if (!value) return

  try {
    await navigator.clipboard.writeText(value)
    copiedGatewayField.value = field
    setTimeout(() => {
      if (copiedGatewayField.value === field) copiedGatewayField.value = null
    }, 1600)
  } catch {
    copiedGatewayField.value = null
  }
}

onBeforeUnmount(() => {
  if (gatewayConfigSaveTimer) {
    clearTimeout(gatewayConfigSaveTimer)
    gatewayConfigSaveTimer = undefined
  }
  if (pendingGatewayConfig) void flushGatewayConfig()
})


</script>

<template>
  <div class="m26-clients-layout">
    <main class="m26-clients-workspace">
<!-- Hermes 专享调度配置。 -->
        <div class="m26-hermes-redesign-container">
          <!-- 顶部控制栏：Auto 调度策略与统一网关连接信息。 -->
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

              <div class="m26-panel-right-group" aria-label="统一网关连接信息">
                <div class="m26-connection-field">
                  <span class="m26-connection-label">统一地址</span>
                  <code class="m26-connection-value" :title="gatewayConnection?.base_url || (gatewayConfigLoaded ? '连接信息读取失败' : '连接信息加载中')">
                    {{ gatewayConnection?.base_url || (gatewayConfigLoaded ? '连接信息读取失败' : '连接信息加载中…') }}
                  </code>
                  <button
                    type="button"
                    class="m26-connection-copy"
                    :disabled="!gatewayConnection?.base_url"
                    :title="copiedGatewayField === 'address' ? '已复制地址' : '复制地址'"
                    @click="copyGatewayField('address')"
                  >
{{ copiedGatewayField === 'address' ? '已复制' : '复制' }}
</button>
                </div>
                <div class="m26-connection-field">
                  <span class="m26-connection-label">API Key</span>
                  <code class="m26-connection-value" :title="gatewayConnection?.api_key || (gatewayConfigLoaded ? '连接信息读取失败' : '连接信息加载中')">
                    {{ gatewayConnection?.api_key || (gatewayConfigLoaded ? '连接信息读取失败' : '连接信息加载中…') }}
                  </code>
                  <button
                    type="button"
                    class="m26-connection-copy"
                    :disabled="!gatewayConnection?.api_key"
                    :title="copiedGatewayField === 'key' ? '已复制 Key' : '复制 Key'"
                    @click="copyGatewayField('key')"
                  >
{{ copiedGatewayField === 'key' ? '已复制' : '复制' }}
</button>
                </div>
                <span v-if="gatewayConfigSaveState !== 'saved'" class="m26-config-sync-state" :class="gatewayConfigSaveState" aria-live="polite">
                  {{ gatewayConfigSaveState === 'saving' ? '配置同步中…' : gatewayConfigSaveState === 'error' ? '配置同步失败' : '配置加载中…' }}
                </span>
              </div>
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
                        ? `${effectiveHermesAllowedModels.length}/${hermesAvailableModels.length}`
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

.m26-clients-workspace {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 0 2px 20px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.m26-clients-workspace:has(.m26-hermes-redesign-container) {
  overflow-y: hidden;
  padding-bottom: 14px;
  gap: 8px;
}

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

.m26-tab-logo-box.hermes {
  background: #f8fafc;
  border: 1px solid #e2e8f0;
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

.m26-hero-content {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
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

.m26-mode-strip-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.m26-mode-strip-label {
  font-size: 12px;
  font-weight: 700;
  color: var(--m26-text);
  padding-right: 10px;
  border-right: 1px solid var(--m26-border);
  white-space: nowrap;
}

.m26-mode-strip-copy {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.m26-mode-strip-copy strong {
  font-size: 12px;
  color: var(--m26-text);
  white-space: nowrap;
}

.m26-mode-strip-copy span {
  font-size: 11px;
  color: var(--m26-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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

.m26-mode-dot {
  width: 7px;
  height: 7px;
  border-radius: 999px;
  background: #94a3b8;
  box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
}

.m26-mode-dot.direct {
  background: #0ea5e9;
}

.m26-mode-dot.gateway {
  background: #8b5cf6;
}

.m26-mode-btn.active .m26-mode-dot {
  box-shadow: 0 0 0 3px rgba(79, 70, 229, 0.1);
}

.m26-mode-btn.disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.m26-mode-subtext {
  font-size: 10px;
  color: var(--m26-text-muted);
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

.m26-profile-active-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #94a3b8;
}

.m26-profile-active-dot.active {
  background: #10b981;
  box-shadow: 0 0 0 3px #d1fae5;
}

.m26-config-status-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 4px 8px;
  border-radius: 999px;
  font-size: 10.5px;
  font-weight: 600;
  border: 1px solid var(--m26-border);
  background: #f8fafc;
  color: var(--m26-text-muted);
}

.m26-config-status-pill.ready {
  color: #047857;
  border-color: #a7f3d0;
  background: #ecfdf5;
}

.m26-config-status-pill.warning {
  color: #a16207;
  border-color: #fde68a;
  background: #fffbeb;
}

.m26-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 999px;
  background: #94a3b8;
}

.m26-config-status-pill.ready .m26-status-dot {
  background: #10b981;
}

.m26-config-status-pill.warning .m26-status-dot {
  background: #f59e0b;
}

.m26-empty-config-state {
  padding: 18px;
  border: 1px dashed #cbd5e1;
  border-radius: 8px;
  background: #f8fafc;
}

.m26-empty-config-title {
  color: var(--m26-text);
  font-size: 12px;
  font-weight: 650;
}

.m26-empty-config-desc {
  max-width: 720px;
  margin-top: 4px;
  color: var(--m26-text-muted);
  font-size: 11px;
  line-height: 1.5;
}

.m26-select-emphasis {
  border-color: #c7d2fe;
  background: #fcfcff;
}

.m26-input-model {
  height: 38px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
}

.m26-direct-channel-meta {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 8px 10px;
  border: 1px solid var(--m26-border);
  border-radius: 7px;
  background: #fafafa;
  min-width: 0;
}

.m26-direct-meta-item {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.m26-direct-meta-label {
  color: var(--m26-text-muted);
  font-size: 10.5px;
  white-space: nowrap;
}

.m26-direct-meta-item code {
  color: var(--m26-text-secondary);
  font-size: 10.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-channel-health {
  font-size: 10.5px;
  color: var(--m26-text-muted);
}

.m26-channel-health.healthy {
  color: #059669;
}

.m26-channel-health.degraded {
  color: #d97706;
}

.m26-model-suggestions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
  margin-top: 5px;
}

.m26-model-suggestion {
  max-width: 220px;
  padding: 3px 7px;
  border: 1px solid var(--m26-border);
  border-radius: 5px;
  background: #ffffff;
  color: var(--m26-text-secondary);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}

.m26-model-suggestion:hover,
.m26-model-suggestion.selected {
  border-color: #a5b4fc;
  background: #eef2ff;
  color: #4338ca;
}

.m26-model-more {
  color: var(--m26-text-muted);
  font-size: 10px;
}

.m26-catalog-summary-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
  padding-bottom: 9px;
  border-bottom: 1px solid #e2e8f0;
}

.m26-catalog-title {
  font-size: 12px;
  font-weight: 650;
  color: var(--m26-text);
}

.m26-catalog-subtitle,
.m26-catalog-note {
  color: var(--m26-text-muted);
  font-size: 10px;
  line-height: 1.45;
}

.m26-catalog-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 24px;
  height: 20px;
  padding: 0 6px;
  border-radius: 999px;
  background: #e0e7ff;
  color: #4338ca;
  font-size: 10px;
  font-weight: 700;
}

.m26-catalog-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
  max-height: 168px;
  margin: 10px 0;
  overflow: auto;
}

.m26-catalog-list code {
  padding: 4px 6px;
  border-radius: 4px;
  background: #ffffff;
  color: var(--m26-text-secondary);
  font-size: 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-catalog-empty {
  padding: 18px 4px;
  color: var(--m26-text-muted);
  font-size: 11px;
  text-align: center;
}

.m26-catalog-note {
  margin-top: auto;
  padding-top: 8px;
}

.m26-footer-hint {
  color: var(--m26-text-muted);
  font-size: 10.5px;
  text-align: right;
}

@media (max-width: 760px) {

  .m26-mode-strip-left {
    width: 100%;
  }

  .m26-mode-segmented {
    width: 100%;
  }

  .m26-mode-btn {
    flex: 1;
    justify-content: center;
  }

  .m26-footer-hint {
    text-align: left;
  }
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

.m26-model-chip-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11.5px;
  font-weight: 600;
  color: var(--m26-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-custom-model-input-row {
  display: flex;
  gap: 8px;
  align-items: center;
}


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


.m26-hermes-redesign-container {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  flex: 1;
  gap: 10px;
  overflow: hidden;
}


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
  display: grid;
  grid-template-columns: minmax(250px, 0.8fr) minmax(420px, 1.2fr) auto;
  align-items: center;
  gap: 5px 10px;
  flex: 0 1 820px;
  min-width: 0;
}

.m26-connection-field {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  height: 30px;
  padding: 0 7px;
  border: 1px solid #dbe3ef;
  border-radius: 7px;
  background: #f8fafc;
}

.m26-connection-label {
  flex-shrink: 0;
  color: #64748b;
  font-size: 10px;
  font-weight: 600;
  white-space: nowrap;
}

.m26-connection-value {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: #334155;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.m26-connection-copy {
  flex-shrink: 0;
  border: 0;
  padding: 3px 5px;
  border-radius: 5px;
  color: #2563eb;
  background: #eff6ff;
  font-size: 10px;
  cursor: pointer;
}

.m26-connection-copy:hover:not(:disabled) {
  background: #dbeafe;
}

.m26-connection-copy:disabled {
  color: #94a3b8;
  cursor: not-allowed;
}

.m26-config-sync-state {
  color: #15803d;
  font-size: 10px;
  white-space: nowrap;
}

.m26-config-sync-state.saving {
  color: #64748b;
}

.m26-config-sync-state.error {
  color: #b91c1c;
}


@media (max-width: 1200px) {
  .m26-bottom-panel-main {
    flex-wrap: wrap;
    gap: 10px;
  }

  .m26-panel-left-group,
  .m26-panel-right-group {
    flex: 1 1 100%;
  }

  .m26-panel-left-group {
    flex-wrap: wrap;
  }

  .m26-panel-right-group {
    grid-template-columns: minmax(250px, 0.8fr) minmax(420px, 1.2fr) auto;
  }
}

@media (max-width: 860px) {
  .m26-panel-right-group {
    grid-template-columns: minmax(0, 1fr);
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

.m26-dropdown-empty {
  font-size: 12px;
  color: #94a3b8;
  text-align: center;
  padding: 20px 10px;
}


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
