// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ChannelToolbar from './ChannelToolbar.vue'

const props = {
  search: '',
  filterStatus: 'all' as const,
  totalCount: 4,
  healthyCount: 2,
  degradedCount: 1,
  errorCount: 1,
  disabledCount: 0,
}

describe('ChannelToolbar', () => {
  it('emits search, filter and add actions', async () => {
    const wrapper = mount(ChannelToolbar, { props })

    await wrapper.get('input').setValue('openai')
    const healthyFilter = wrapper
      .findAll('button')
      .find((button) => button.text().includes('就绪'))
    await healthyFilter?.trigger('click')
    await wrapper.get('button[aria-label="添加渠道"]').trigger('click')

    expect(wrapper.emitted('update:search')).toEqual([['openai']])
    expect(wrapper.emitted('update:filterStatus')).toEqual([['healthy']])
    expect(wrapper.emitted('add-channel')).toHaveLength(1)
  })

  it('exposes a search focus helper and clears an existing query', async () => {
    const wrapper = mount(ChannelToolbar, {
      props: { ...props, search: 'existing' },
      attachTo: document.body,
    })
    const input = wrapper.get('input').element as HTMLInputElement

    await wrapper.vm.focusSearch()

    expect(document.activeElement).toBe(input)
    expect(wrapper.find('button[title="清空搜索"]').exists()).toBe(true)
    await wrapper.get('button[title="清空搜索"]').trigger('click')
    expect(wrapper.emitted('update:search')).toContainEqual([''])

    wrapper.unmount()
  })

  it('does not render zero-count status filters', () => {
    const wrapper = mount(ChannelToolbar, { props })

    expect(wrapper.text()).toContain('就绪')
    expect(wrapper.text()).toContain('高延迟')
    expect(wrapper.text()).toContain('异常')
    expect(wrapper.text()).not.toContain('停用')
  })
})
