// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import ChannelKeyManager from './ChannelKeyManager.vue'
import { keyRequest } from '@/services/channelKeys'
vi.mock('@/services/channelKeys', () => ({ keyRequest: vi.fn() }))
const key = { id: 1, channel_id: 7, name: 'Primary', masked_secret: '••••abcd', enabled: true, position: 0, auth_invalid: false, cooldown_until: null, last_error: '', attempts: 0, successes: 0, rate_limits: 0, prompt_tokens: 0, completion_tokens: 0 }
beforeEach(() => { vi.mocked(keyRequest).mockReset().mockResolvedValue([key, { ...key, id: 2, name: 'Backup' }]) })
describe('ChannelKeyManager', () => {
  it('embeds only the edited supplier without a second selector or close button', async () => {
    const wrapper = mount(ChannelKeyManager, { props: { channels: [{ gatewayId: 7, name: 'Supplier' }, { gatewayId: 8, name: 'Edited' }], fixedChannelId: 8, embedded: true } })
    await flushPromises()
    expect(keyRequest).toHaveBeenLastCalledWith(8, 'GET', undefined, undefined)
    expect(wrapper.find('#key-channel').exists()).toBe(false)
    expect(wrapper.find('button[aria-label="关闭"]').exists()).toBe(false)
    expect(wrapper.text()).toContain('立即生效')
    wrapper.unmount()
  })
  it('disables paid tests when the parent channel is disabled', async () => {
    const wrapper = mount(ChannelKeyManager, { props: { channels: [{ gatewayId: 7, name: 'Supplier', enabled: false }] } })
    await flushPromises()
    expect(wrapper.text()).toContain('当前渠道已停用')
    expect(wrapper.findAll('button').filter(b => b.text().startsWith('测试')).every(b => b.attributes('disabled') !== undefined)).toBe(true)
    wrapper.unmount()
  })
  it('loads metadata, deduplicates input and clears secrets only after success', async () => {
    const wrapper = mount(ChannelKeyManager, { props: { channels: [{ gatewayId: 7, name: 'Supplier' }] } })
    await flushPromises()
    expect(wrapper.text()).toContain('Primary')
    await wrapper.get('textarea').setValue('test-secret\ntest-secret\nsecond-secret')
    await wrapper.get('textarea').element.form?.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
    await flushPromises()
    expect(keyRequest).toHaveBeenLastCalledWith(7, 'POST', { keys: [{ name: 'Key 3', secret: 'test-secret' }, { name: 'Key 4', secret: 'second-secret' }] }, undefined)
    expect(wrapper.get('textarea').element.value).toBe('')
    wrapper.unmount()
  })
  it('requires confirmation to delete and retains the list on errors', async () => {
    const wrapper = mount(ChannelKeyManager, { props: { channels: [{ gatewayId: 7, name: 'Supplier' }] } })
    await flushPromises()
    await wrapper.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    expect(keyRequest).toHaveBeenCalledTimes(1)
    vi.mocked(keyRequest).mockRejectedValueOnce(new Error('无法删除'))
    await wrapper.findAll('button').find(b => b.text() === '确认删除')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('无法删除')
    expect(wrapper.text()).toContain('Primary')
    wrapper.unmount()
  })
})
