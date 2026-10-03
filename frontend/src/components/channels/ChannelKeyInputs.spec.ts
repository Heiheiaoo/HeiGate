// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelKeyInputs from './ChannelKeyInputs.vue'

describe('ChannelKeyInputs', () => {
  it('adds masked rows and emits all keys together', async () => {
    const wrapper = mount(ChannelKeyInputs, { props: { modelValue: '' } })
    await wrapper.get('input').setValue('key-a')
    await wrapper.findAll('button').find(b => b.text() === '添加一把 Key')!.trigger('click')
    await wrapper.get('input[aria-label="Key 2"]').setValue('key-b')
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual(['key-a\nkey-b'])
    expect(wrapper.findAll('input').every(input => input.attributes('type') === 'password')).toBe(true)
    await wrapper.get('button[aria-label="移除 Key 1"]').trigger('click')
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual(['key-b'])
  })
  it('deduplicates batch paste and clears secrets when the parent resets', async () => {
    const wrapper = mount(ChannelKeyInputs, { props: { modelValue: 'key-a' } })
    await wrapper.get('textarea').setValue('key-a\r\n key-b \nkey-b')
    await wrapper.findAll('button').find(b => b.text() === '加入列表并去重')!.trigger('click')
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual(['key-a\nkey-b'])
    expect(wrapper.get('textarea').element.value).toBe('')
    await wrapper.setProps({ modelValue: '' })
    expect(wrapper.findAll('input')).toHaveLength(1)
    expect(wrapper.get('input').element.value).toBe('')
  })
})
