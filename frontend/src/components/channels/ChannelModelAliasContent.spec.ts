// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelModelAliasContent from './ChannelModelAliasContent.vue'
import type { AggregateModelAliasItem, AggregateModelChannelDetail, AggregateModelView } from './types'

const model: AggregateModelView = {
  name: 'gpt-4o',
  aliases: ['gpt-4o-2024-08-06'],
  providerCount: 2,
}

const channels: AggregateModelChannelDetail[] = [
  { name: 'Primary', status: 'healthy', latency: 320, protocol: 'openai-compatible', upstreamModel: 'gpt-4o' },
  { name: 'Backup', status: 'degraded', latency: 1500, protocol: 'anthropic', upstreamModel: 'gpt-4o-2024-08-06' },
]

const aliases: AggregateModelAliasItem[] = [
  { alias: 'gpt-4o-2024-08-06', channels: [channels[1]] },
]

function mountContent() {
  return mount(ChannelModelAliasContent, {
    props: {
      model,
      family: { label: 'OpenAI', color: '#059669', bg: '#ecfdf5', border: '#a7f3d0' },
      aliases,
      channelDetails: channels,
      search: '',
      copiedText: '',
      mappedAliases: { 'gpt-4o-2024-08-06': 'gpt-4o' },
    },
  })
}

describe('ChannelModelAliasContent', () => {
  it('renders alias guidance and channel rewrite mapping', () => {
    const wrapper = mountContent()

    expect(wrapper.text()).toContain('gpt-4o')
    expect(wrapper.text()).toContain('gpt-4o-2024-08-06')
    expect(wrapper.text()).toContain('别名重写')
    expect(wrapper.text()).toContain('同名直通')
    expect(wrapper.text()).toContain('延迟较高')
    expect(wrapper.text()).toContain('手动归类')
  })

  it('emits search, copy, mapping, speed-test, classify, and close actions', async () => {
    const wrapper = mountContent()

    await wrapper.get('input').setValue('backup')
    await wrapper.get('button[title="复制模型名: gpt-4o"]').trigger('click')
    await wrapper.get('button[title="复制全部上游别名（每行一个）"]').trigger('click')
    await wrapper.get('button[title="复制别名: gpt-4o-2024-08-06"]').trigger('click')
    await wrapper.get('button[title="解除手动归类，恢复为独立模型"]').trigger('click')
    await wrapper.get('button[title="并发探活此模型全部供应商的可用性与延迟"]').trigger('click')
    await wrapper.get('button[title="将其他渠道模型手动归并到此聚合模型"]').trigger('click')
    await wrapper.get('button[aria-label="关闭"]').trigger('click')

    expect(wrapper.emitted('update:search')).toContainEqual(['backup'])
    expect(wrapper.emitted('copy-model')).toHaveLength(1)
    expect(wrapper.emitted('copy-aliases')).toHaveLength(1)
    expect(wrapper.emitted('copy-alias')).toContainEqual(['gpt-4o-2024-08-06'])
    expect(wrapper.emitted('remove-mapping')).toEqual([['gpt-4o-2024-08-06']])
    expect(wrapper.emitted('open-speed-test')).toHaveLength(1)
    expect(wrapper.emitted('open-classify')).toHaveLength(1)
    expect(wrapper.emitted('close')).toHaveLength(1)
  })
})
