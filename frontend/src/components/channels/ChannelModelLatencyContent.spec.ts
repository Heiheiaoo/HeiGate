// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelModelLatencyContent from './ChannelModelLatencyContent.vue'
import type { Channel, ModelLatencyRow } from './types'

const channel: Channel = {
  name: 'Primary',
  url: 'https://api.example.test/v1',
  protocol: 'openai-compatible',
  status: 'healthy',
  latency: 900,
  ttft: 250,
  models: 2,
  lastCheck: '刚刚',
  accent: '#3b82f6',
  enabled: true,
  modelNames: ['gpt-4o', 'gpt-4o-mini'],
}

const rows: ModelLatencyRow[] = [
  {
    name: 'gpt-4o',
    selected: true,
    checkedAtLabel: '刚刚',
    latencyClass: 'fast',
    latencyDisplay: '0.25s',
    latencyTooltip: '模型调用成功',
    failed: false,
    injectionClass: 'clean',
    injectionTooltip: '输入统计正常',
    injectionLabel: '正常',
    probing: false,
    checkingInjection: false,
    copied: false,
  },
  {
    name: 'gpt-4o-mini',
    selected: false,
    checkedAtLabel: '',
    latencyClass: 'timeout',
    latencyDisplay: '—',
    latencyTooltip: '探活异常：timeout',
    failed: true,
    injectionClass: 'unknown',
    injectionTooltip: '尚未检测',
    injectionLabel: '未检测',
    probing: false,
    checkingInjection: false,
    copied: false,
  },
]

function mountContent() {
  return mount(ChannelModelLatencyContent, {
    props: {
      channel,
      rows,
      selectedCount: 1,
      allSelected: false,
      someSelected: true,
      probingAll: false,
      checkingAllInjections: false,
    },
  })
}

describe('ChannelModelLatencyContent', () => {
  it('renders measured latency and failed model details', () => {
    const wrapper = mountContent()

    expect(wrapper.text()).toContain('0.25s')
    expect(wrapper.find('[aria-label="当前主模型"]').exists()).toBe(false)
    expect(wrapper.get('.m26-model-lat-pill').attributes('title')).toBe('模型调用成功')
    expect(wrapper.findAll('.m26-model-lat-pill')[1].attributes('title')).toBe('探活异常：timeout')
    expect(wrapper.text()).toContain('未检测')
  })

  it('emits batch, selection, and row actions with the model context', async () => {
    const wrapper = mountContent()
    const batchProbe = wrapper.findAll('button').find((button) => button.attributes('aria-label')?.includes('批量探活'))
    await batchProbe?.trigger('click')
    await wrapper.findAll('input[type="checkbox"]')[2].trigger('change')
    await wrapper.get('button[aria-label="探活 gpt-4o-mini"]').trigger('click')

    expect(wrapper.emitted('batch-action')).toEqual([['probe']])
    expect(wrapper.emitted('toggle-selection')).toEqual([['gpt-4o-mini']])
    expect(wrapper.emitted('probe-model')).toEqual([['gpt-4o-mini']])
  })

  it('emits injection detail and copy actions without selecting the card', async () => {
    const wrapper = mountContent()
    await wrapper.get('.m26-model-inject-pill').trigger('click')
    await wrapper.get('button.m26-model-row-copy').trigger('click')

    expect(wrapper.emitted('open-injection-detail')).toEqual([['gpt-4o']])
    expect(wrapper.emitted('copy-model')).toEqual([['gpt-4o']])
    expect(wrapper.emitted('card-click')).toBeUndefined()
  })
})
