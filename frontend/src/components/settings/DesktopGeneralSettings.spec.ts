// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import DesktopGeneralSettings from './DesktopGeneralSettings.vue'

describe('DesktopGeneralSettings', () => {
  it('renders switch with correct checked state and emits change', async () => {
    const wrapper = mount(DesktopGeneralSettings, {
      props: {
        hideDockIcon: false,
        supported: true,
      },
    })

    const input = wrapper.find<HTMLInputElement>('input.m26-switch-input')
    expect(input.exists()).toBe(true)
    expect(input.element.checked).toBe(false)
    expect(wrapper.text()).toContain('是否隐藏Dock图标（仅 macOS）')

    input.element.checked = true
    await input.trigger('change')

    expect(wrapper.emitted('update:hideDockIcon')).toEqual([[true]])

    await wrapper.setProps({ hideDockIcon: true })
    expect(input.element.checked).toBe(true)
  })
})
