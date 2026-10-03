// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelProviderSpeedTestContent from './ChannelProviderSpeedTestContent.vue'
import type { Channel, ModelProviderSpeedTestRow } from './types'

const primary: Channel = {
  name: 'Primary',
  url: 'https://primary.example.test/v1',
  protocol: 'openai-compatible',
  status: 'healthy',
  latency: 420,
  ttft: 180,
  models: 1,
  lastCheck: '刚刚',
  accent: '#3b82f6',
  enabled: true,
  modelNames: ['gpt-4o'],
}

const backup: Channel = {
  ...primary,
  name: 'Backup',
  url: 'https://backup.example.test/v1',
  latency: null,
  status: 'offline',
  modelNames: ['gpt-4o-mini'],
}

const rows: ModelProviderSpeedTestRow[] = [
  {
    channel: primary,
    upstreamModel: 'gpt-4o',
    protocolLabel: 'OpenAI Chat',
    latencyDisplay: '0.18s',
    latencyClass: 'fast',
    isProbing: false,
    isFailed: false,
    tooltip: '模型调用成功',
    preferred: true,
  },
  {
    channel: backup,
    upstreamModel: 'gpt-4o-mini',
    protocolLabel: 'Anthropic',
    latencyDisplay: '—',
    latencyClass: 'timeout',
    isProbing: false,
    isFailed: true,
    tooltip: '探活异常：timeout',
    preferred: false,
  },
]

function mountContent() {
  return mount(ChannelProviderSpeedTestContent, {
    props: {
      modelName: 'gpt-4o',
      modelFamily: { label: 'GPT', color: '#2563eb', bg: '#eff6ff', border: '#bfdbfe' },
      providers: rows,
      query: '',
      fastest: { channelName: 'Primary', latency: 180 },
      probingAll: false,
      copied: false,
    },
  })
}

describe('ChannelProviderSpeedTestContent', () => {
  it('renders provider comparison and fastest-provider state', () => {
    const wrapper = mountContent()

    expect(wrapper.text()).toContain('gpt-4o')
    expect(wrapper.text()).toContain('Primary')
    expect(wrapper.text()).toContain('Backup')
    expect(wrapper.text()).toContain('优选首选')
    expect(wrapper.text()).toContain('最优: Primary')
    expect(wrapper.text()).toContain('0.18s')
  })

  it('emits search, copy, probe-all and provider actions', async () => {
    const wrapper = mountContent()
    await wrapper.get('input').setValue('backup')
    await wrapper.get('button[title="复制模型名: gpt-4o"]').trigger('click')
    await wrapper.get('button[title="重新对该模型的所有供应商发起并发真实探活"]').trigger('click')
    await wrapper.findAll('button[title="单独重新探活此供应商"]')[1].trigger('click')
    await wrapper.findAll('button[title="查看此渠道完整诊断信息"]')[1].trigger('click')

    expect(wrapper.emitted('update:query')).toContainEqual(['backup'])
    expect(wrapper.emitted('copy-model')).toHaveLength(1)
    expect(wrapper.emitted('probe-all')).toHaveLength(1)
    expect(wrapper.emitted('probe-provider')?.[0]?.[0]).toEqual(rows[1])
    expect(wrapper.emitted('open-diagnostic')?.[0]?.[0]).toEqual(rows[1])
  })

  it('shows an empty search state and supports closing', async () => {
    const wrapper = mount(ChannelProviderSpeedTestContent, {
      props: {
        modelName: 'missing-model',
        modelFamily: { label: 'Other', color: '#111827', bg: '#f9fafb', border: '#d1d5db' },
        providers: [],
        query: 'missing',
        fastest: null,
        probingAll: false,
        copied: false,
      },
    })

    expect(wrapper.text()).toContain('未找到匹配 “missing”')
    await wrapper.get('button[aria-label="关闭"]').trigger('click')
    expect(wrapper.emitted('close')).toHaveLength(1)
  })
})
