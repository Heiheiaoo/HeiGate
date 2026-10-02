// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import RoutingSettings from './RoutingSettings.vue'

describe('RoutingSettings', () => {
  it.each(['smart_quality', 'latency', 'round_robin'] as const)('renders and selects %s without changing the controlled value', async (strategy) => {
    const strategies = ['smart_quality', 'latency', 'round_robin']
    const wrapper = mount(RoutingSettings, { props: { routingStrategy: 'latency' } })
    const cards = wrapper.findAll('.m26-route-card')
    expect(cards).toHaveLength(3)
    expect(cards[1].classes()).toContain('selected')
    await cards[strategies.indexOf(strategy)].trigger('click')
    expect(wrapper.emitted('select')).toEqual([[strategy]])
    expect(cards[1].classes()).toContain('selected')
    await wrapper.setProps({ routingStrategy: strategy })
    expect(wrapper.findAll('.selected')).toHaveLength(1)
    expect(cards[strategies.indexOf(strategy)].classes()).toContain('selected')
  })
})
