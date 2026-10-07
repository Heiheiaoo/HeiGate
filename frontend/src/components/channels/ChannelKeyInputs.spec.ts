// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelKeyInputs, { type ChannelKeyDraft } from './ChannelKeyInputs.vue'

describe('ChannelKeyInputs', () => {
  it('shows a short label without routing instructions', () => {
    const wrapper = mount(ChannelKeyInputs, { props: { modelValue: [] } })
    expect(wrapper.get('label').text()).toBe('API Key')
    expect(wrapper.text()).not.toContain('冷却')
    expect(wrapper.find('textarea').exists()).toBe(false)
  })
  it('adds masked rows and emits all keys together', async () => {
    const wrapper = mount(ChannelKeyInputs, { props: { modelValue: [] } })
    await wrapper.get('input[aria-label="Key 1"]').setValue('key-a')
    await wrapper.findAll('button').find(b => b.text() === '添加 Key')!.trigger('click')
    await wrapper.get('input[aria-label="Key 2"]').setValue('key-b')
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual<ChannelKeyDraft[][]>([[
      { name: 'Key 1', secret: 'key-a' },
      { name: 'Key 2', secret: 'key-b' },
    ]])
    expect(wrapper.findAll('.key-secret-input').every(input => input.attributes('type') === 'password')).toBe(true)
    await wrapper.get('button[aria-label="移除 Key 1"]').trigger('click')
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual<ChannelKeyDraft[][]>([[
      { name: 'Key 2', secret: 'key-b' },
    ]])
  })
  it('clears secrets when the parent resets', async () => {
    const wrapper = mount(ChannelKeyInputs, { props: { modelValue: [{ name: 'Existing', secret: 'key-a' }] } })
    await wrapper.setProps({ modelValue: [] })
    expect(wrapper.findAll('input')).toHaveLength(2)
    expect((wrapper.get('input[aria-label="Key 1 名称"]').element as HTMLInputElement).value).toBe('Key 1')
    expect((wrapper.get('input[aria-label="Key 1"]').element as HTMLInputElement).value).toBe('')
  })
})
