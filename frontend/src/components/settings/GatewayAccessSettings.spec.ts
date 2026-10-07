// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import GatewayAccessSettings from './GatewayAccessSettings.vue'

const props = {
  gatewayBaseUrl: 'http://127.0.0.1:28888',
  gatewayApiKey: 'test-only-gateway-secret',
  maskedOverviewApiKey: 'test-onl••••••••secret',
  showApiKey: false,
}

describe('GatewayAccessSettings', () => {
  it('masks the secret until the parent enables visibility and can hide it again', async () => {
    const wrapper = mount(GatewayAccessSettings, { props })
    expect(wrapper.text()).toContain(props.gatewayBaseUrl)
    expect(wrapper.text()).toContain(props.maskedOverviewApiKey)
    expect(wrapper.html()).not.toContain(props.gatewayApiKey)
    await wrapper.get('button[title="显示完整 API Key"]').trigger('click')
    expect(wrapper.emitted('update:showApiKey')).toEqual([[true]])
    expect(wrapper.html()).not.toContain(props.gatewayApiKey)
    await wrapper.setProps({ showApiKey: true })
    expect(wrapper.text()).toContain(props.gatewayApiKey)
    await wrapper.get('button[title="隐藏 API Key"]').trigger('click')
    expect(wrapper.emitted('update:showApiKey')).toEqual([[true], [false]])
    await wrapper.setProps({ showApiKey: false })
    expect(wrapper.html()).not.toContain(props.gatewayApiKey)
  })

  it('emits copy intent from both value text and buttons without sending secrets', async () => {
    const wrapper = mount(GatewayAccessSettings, { props })
    for (const title of ['点击复制 Base URL', '复制 Base URL', '点击复制 API Key', '复制 API Key']) {
      await wrapper.get(`[title="${title}"]`).trigger('click')
    }
    expect(wrapper.emitted('copy-url')).toEqual([[], []])
    expect(wrapper.emitted('copy-key')).toEqual([[], []])
  })

  it('renders the empty-key fallback', async () => {
    const wrapper = mount(GatewayAccessSettings, { props: { ...props, gatewayApiKey: '', maskedOverviewApiKey: '' } })
    expect(wrapper.text()).toContain('••••••••')
    await wrapper.setProps({ showApiKey: true })
    expect(wrapper.text()).toContain('暂无 Key')
  })
})
