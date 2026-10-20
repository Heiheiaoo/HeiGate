// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelModelManager from './ChannelModelManager.vue'

function mountManager(overrides: Partial<InstanceType<typeof ChannelModelManager>['$props']> = {}) {
  return mount(ChannelModelManager, {
    props: {
      modelNames: ['gpt-4o', 'gpt-4o-mini'],
      ...overrides,
    },
  })
}

describe('ChannelModelManager', () => {
  it('keeps model labels concise without explanatory paragraphs', () => {
    const wrapper = mountManager()
    expect(wrapper.text()).not.toContain('周期探活')
    expect(wrapper.text()).not.toContain('可用模型')
    expect(wrapper.get('.m26-add-bar .m26-mgr-fetch-btn').text()).toBe('获取模型')
    expect(wrapper.get('.m26-count-pill').text()).toBe('2')
    expect(wrapper.get('.m26-add-bar input').attributes('placeholder')).toContain('输入模型名称')
  })
  it('adds pasted model names and removes duplicates', async () => {
    const wrapper = mountManager()
    const input = wrapper.get('.m26-add-bar input')

    await input.setValue('gpt-4o-mini, claude-3, gpt-4o')
    await input.trigger('keydown', { key: 'Enter' })

    expect(wrapper.emitted('update:modelNames')).toEqual([
      [['gpt-4o', 'gpt-4o-mini', 'claude-3']],
    ])
  })

  it('removes a model without assigning a special model slot', async () => {
    const wrapper = mountManager()

    await wrapper.get('button[aria-label="移除模型 gpt-4o"]').trigger('click')

    expect(wrapper.emitted('update:modelNames')).toEqual([[['gpt-4o-mini']]])
  })

  it('emits a fetch request without selecting a special model slot', async () => {
    const wrapper = mountManager()

    await wrapper.get('.m26-mgr-fetch-btn').trigger('click')

    expect(wrapper.emitted('fetchUpstream')).toHaveLength(1)
  })

  it('deduplicates text mode input', async () => {
    const wrapper = mountManager({ modelNames: ['removed-model'] })

    const textModeToggle = wrapper
      .findAll('button')
      .find((button) => button.text().includes('多行文本'))
    await textModeToggle?.trigger('click')
    await wrapper.get('textarea').setValue('claude-3\nclaude-3\ngpt-4o')

    expect(wrapper.emitted('update:modelNames')).toContainEqual([['claude-3', 'gpt-4o']])
  })
})
