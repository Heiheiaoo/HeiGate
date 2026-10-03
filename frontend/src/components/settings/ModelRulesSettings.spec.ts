// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ModelRulesSettings from './ModelRulesSettings.vue'

const props = {
  newMappingSource: '',
  newMappingTarget: '',
  allDetectedChannelModels: ['upstream-model'],
  existingCanonicalModelNames: ['canonical-model'],
  customModelMappings: {},
  appVersion: '2.0.0',
}

describe('ModelRulesSettings', () => {
  it('renders empty rules, model suggestions and the supplied version', () => {
    const wrapper = mount(ModelRulesSettings, { props })
    expect(wrapper.text()).toContain('暂无自定义归类规则')
    expect(wrapper.text()).toContain('v2.0.0')
    expect(wrapper.get('datalist#source-model-suggestions option').attributes('value')).toBe('upstream-model')
    expect(wrapper.get('datalist#target-model-suggestions option').attributes('value')).toBe('canonical-model')
    expect(wrapper.emitted()).toEqual({})
  })

  it('emits draft edits and only enables adding when both controlled values are nonblank', async () => {
    const wrapper = mount(ModelRulesSettings, { props })
    const button = wrapper.get<HTMLButtonElement>('button')
    expect(button.element.disabled).toBe(true)
    const [source, target] = wrapper.findAll('input')
    await source.setValue(' upstream-model ')
    await target.setValue(' canonical-model ')
    expect(wrapper.emitted('update:newMappingSource')).toEqual([[' upstream-model ']])
    expect(wrapper.emitted('update:newMappingTarget')).toEqual([[' canonical-model ']])
    expect(button.element.disabled).toBe(true)
    for (const [newMappingSource, newMappingTarget] of [[' ', 'model'], ['model', ' ']]) {
      await wrapper.setProps({ newMappingSource, newMappingTarget })
      await button.trigger('click')
      expect(button.element.disabled).toBe(true)
      expect(wrapper.emitted('add-mapping')).toBeUndefined()
    }
    await wrapper.setProps({ newMappingSource: ' upstream-model ', newMappingTarget: ' canonical-model ' })
    expect(button.element.disabled).toBe(false)
    await button.trigger('click')
    expect(wrapper.emitted('add-mapping')).toEqual([[]])
    expect(wrapper.props('newMappingSource')).toBe(' upstream-model ')
    await wrapper.setProps({ newMappingSource: '', newMappingTarget: '' })
    expect(source.element.value).toBe('')
    expect(target.element.value).toBe('')
  })

  it('emits the selected source for removal without changing rules and escapes model text', async () => {
    const mappings = { '<script>bad</script>': 'canonical-model', other: 'other-target' }
    const wrapper = mount(ModelRulesSettings, { props: { ...props, customModelMappings: mappings } })
    expect(wrapper.text()).not.toContain('暂无自定义归类规则')
    expect(wrapper.find('script').exists()).toBe(false)
    expect(wrapper.text()).toContain('<script>bad</script>')
    await wrapper.findAll('[title="删除此条归类映射规则"]')[1].trigger('click')
    expect(wrapper.emitted('remove-mapping')).toEqual([['other']])
    expect(wrapper.props('customModelMappings')).toEqual(mappings)
    await wrapper.setProps({ customModelMappings: {}, appVersion: '2.0.0' })
    expect(wrapper.text()).toContain('暂无自定义归类规则')
    expect(wrapper.text()).toContain('v2.0.0')
  })

  it('hides the mapping group when showMappings is false', () => {
    const wrapper = mount(ModelRulesSettings, { props: { ...props, showMappings: false } })
    expect(wrapper.text()).not.toContain('自定义模型归类与别名合并')
    expect(wrapper.text()).toContain('关于 HeiGate')
    expect(wrapper.text()).toContain('v2.0.0')
  })
})
