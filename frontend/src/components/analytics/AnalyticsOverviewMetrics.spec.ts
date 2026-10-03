// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import AnalyticsOverviewMetrics from './AnalyticsOverviewMetrics.vue'

function mountContent(overrides: Record<string, unknown> = {}) {
  return mount(AnalyticsOverviewMetrics, {
    props: {
      healthyChannels: 2,
      channelCount: 3,
      enabledChannelsCount: 2,
      averageLatency: 420,
      routingStrategy: 'latency',
      modelCount: 4,
      availableModels: 3,
      ...overrides,
    },
  })
}

describe('AnalyticsOverviewMetrics', () => {
  it('renders channel, latency, routing, and model metrics', () => {
    const wrapper = mountContent()
    expect(wrapper.text()).toContain('2/ 3')
    expect(wrapper.text()).toContain('0.42s')
    expect(wrapper.text()).toContain('最低延迟')
    expect(wrapper.text()).toContain('4')
    expect(wrapper.text()).toContain('3 家上游提供')
  })

  it('labels routing strategies and slow latency', () => {
    const wrapper = mountContent({ routingStrategy: 'smart_quality', averageLatency: 1200 })
    expect(wrapper.text()).toContain('智能综合质量')
    expect(wrapper.text()).toContain('1.20s')
    expect(wrapper.text()).toContain('正常负载')
  })
})
