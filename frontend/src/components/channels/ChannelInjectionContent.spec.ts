// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import type { ModelInjectionResult } from '@/services/gatewayApi'
import ChannelInjectionContent from './ChannelInjectionContent.vue'
import type { Channel } from './types'

const channel: Channel = {
  name: 'Primary',
  url: 'https://api.example.test/v1',
  protocol: 'openai-compatible',
  status: 'healthy',
  latency: 400,
  ttft: 180,
  models: 2,
  lastCheck: '刚刚',
  accent: '#3b82f6',
  enabled: true,
  modelNames: ['gpt-4o', 'gpt-4o-mini'],
  injectionState: 'detected',
  injectionTokens: 640,
  injectionCheckedAt: '2026-10-02T10:00:00Z',
}

const results: ModelInjectionResult[] = [
  {
    model: 'gpt-4o',
    state: 'detected',
    prompt_tokens: 640,
    content: 'You are an injected system prompt.',
    checked_at: '2026-10-02T10:00:00Z',
  },
  {
    model: 'gpt-4o-mini',
    state: 'clean',
    prompt_tokens: 42,
    checked_at: '2026-10-02T10:00:01Z',
  },
]

function mountContent(overrides: Partial<InstanceType<typeof ChannelInjectionContent>['$props']> = {}) {
  return mount(ChannelInjectionContent, {
    props: {
      channel,
      results,
      checking: false,
      copiedModelId: '',
      copiedInjectionText: '',
      ...overrides,
    },
  })
}

describe('ChannelInjectionContent', () => {
  it('renders detected and clean result states with prompt content', () => {
    const wrapper = mountContent()

    expect(wrapper.text()).toContain('注入检测 — Primary')
    expect(wrapper.text()).toContain('显著偏高 · 640 tok')
    expect(wrapper.text()).toContain('输入正常')
    expect(wrapper.text()).toContain('You are an injected system prompt.')
    expect(wrapper.text()).toContain('上次检测时间: 2026-10-02 10:00:00')
  })

  it('emits copy and detection actions', async () => {
    const wrapper = mountContent()
    const copyModel = wrapper.findAll('button').find((button) => button.attributes('title') === '复制模型名称')
    const copyContent = wrapper.get('button[title="复制完整注入 Prompt 内容"]')

    await copyModel?.trigger('click')
    await copyContent.trigger('click')
    await wrapper.get('button[title="仅检测首个模型（推荐，速度快且省 Token）"]').trigger('click')
    await wrapper.get('button[title="逐一检测全部模型（耗时可能较长）"]').trigger('click')
    await wrapper.get('button[aria-label="关闭"]').trigger('click')

    expect(wrapper.emitted('copy-model')).toEqual([['gpt-4o']])
    expect(wrapper.emitted('copy-content')).toEqual([[results[0].content]])
    expect(wrapper.emitted('check-one')).toHaveLength(1)
    expect(wrapper.emitted('check-all')).toHaveLength(1)
    expect(wrapper.emitted('close')).toHaveLength(1)
  })

  it('renders an empty state and disables actions while checking', () => {
    const wrapper = mountContent({ results: [], checking: true })

    expect(wrapper.text()).toContain('该渠道尚未执行注入检测')
    expect(wrapper.get('button[title="仅检测首个模型（推荐，速度快且省 Token）"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('button[title="逐一检测全部模型（耗时可能较长）"]').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('全量检测中...')
  })
})
