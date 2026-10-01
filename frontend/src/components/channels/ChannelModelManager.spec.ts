// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelModelManager from './ChannelModelManager.vue'

function mountManager(overrides: Partial<InstanceType<typeof ChannelModelManager>['$props']> = {}) {
  return mount(ChannelModelManager, {
    props: {
      primaryModel: 'gpt-4o',
      modelNames: ['gpt-4o', 'gpt-4o-mini'],
      ...overrides,
    },
  })
}

describe('ChannelModelManager', () => {
  it('adds pasted model names, removes duplicates, and keeps the existing primary', async () => {
    const wrapper = mountManager()
    const input = wrapper.get('.m26-add-bar input')

    await input.setValue('gpt-4o-mini, claude-3, gpt-4o')
    await input.trigger('keydown', { key: 'Enter' })

    expect(wrapper.emitted('update:modelNames')).toEqual([
      [['gpt-4o', 'gpt-4o-mini', 'claude-3']],
    ])
    expect(wrapper.emitted('update:primaryModel')).toBeUndefined()
  })

  it('reassigns the primary model when the current one is removed', async () => {
    const wrapper = mountManager()

    await wrapper.get('.m26-tag-capsule.is-primary .tag-del-btn').trigger('click')

    expect(wrapper.emitted('update:modelNames')).toEqual([[['gpt-4o-mini']]])
    expect(wrapper.emitted('update:primaryModel')).toEqual([['gpt-4o-mini']])
  })

  it('selects another model as primary and emits a fetch request', async () => {
    const wrapper = mountManager()

    await wrapper.get('.tag-set-btn').trigger('click')
    await wrapper.get('.m26-mgr-fetch-btn').trigger('click')

    expect(wrapper.emitted('update:primaryModel')).toEqual([['gpt-4o-mini']])
    expect(wrapper.emitted('fetchUpstream')).toHaveLength(1)
  })

  it('deduplicates text mode input and promotes its first model when needed', async () => {
    const wrapper = mountManager({ primaryModel: 'removed-model', modelNames: ['removed-model'] })

    const textModeToggle = wrapper
      .findAll('button')
      .find((button) => button.text().includes('多行文本'))
    await textModeToggle?.trigger('click')
    await wrapper.get('textarea').setValue('claude-3\nclaude-3\ngpt-4o')

    expect(wrapper.emitted('update:modelNames')).toContainEqual([['claude-3', 'gpt-4o']])
    expect(wrapper.emitted('update:primaryModel')).toContainEqual(['claude-3'])
  })
})
