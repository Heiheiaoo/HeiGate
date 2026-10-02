// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ModelMappingContent from './ModelMappingContent.vue'

describe('ModelMappingContent', () => {
  it('submits a quick classification using the selected canonical target', async () => {
    const wrapper = mount(ModelMappingContent, {
      props: {
        mode: 'classify',
        sourceSuggestions: ['vendor-gpt-4o'],
        targetOptions: ['gpt-4o', 'claude-3-5-sonnet'],
        initialSource: 'vendor-gpt-4o',
        initialTarget: 'gpt-4o',
      },
    })

    const submit = wrapper.findAll('button').find((button) => button.text() === '确认归并')
    await submit?.trigger('click')

    expect(wrapper.emitted('submit')).toEqual([[{ source: 'vendor-gpt-4o', target: 'gpt-4o' }]])
  })

  it('supports a custom target in manager mode and emits removal', async () => {
    const wrapper = mount(ModelMappingContent, {
      props: {
        mode: 'manage',
        sourceSuggestions: ['vendor-gpt-4o'],
        targetOptions: ['gpt-4o'],
        mappings: { 'legacy-model': 'gpt-4o' },
      },
    })

    await wrapper.get('input[list="mapping-source-manage"]').setValue('new-vendor-model')
    await wrapper.get('select').setValue('__custom__')
    await wrapper.get('input[placeholder="目标模型规范名称"]').setValue('new-standard-model')
    await wrapper.findAll('button').find((button) => button.text() === '添加')?.trigger('click')
    await wrapper.get('button[aria-label="解除 legacy-model 的归类映射"]').trigger('click')

    expect(wrapper.emitted('submit')).toEqual([[{ source: 'new-vendor-model', target: 'new-standard-model' }]])
    expect(wrapper.emitted('remove')).toEqual([['legacy-model']])
  })

  it('shows an empty manager state and keeps submit disabled without a source', () => {
    const wrapper = mount(ModelMappingContent, {
      props: {
        mode: 'manage',
        sourceSuggestions: [],
        targetOptions: ['gpt-4o'],
        mappings: {},
      },
    })

    expect(wrapper.text()).toContain('暂无自定义归类规则')
    expect(wrapper.findAll('button').find((button) => button.text() === '添加')?.attributes('disabled')).toBeDefined()
  })
})
