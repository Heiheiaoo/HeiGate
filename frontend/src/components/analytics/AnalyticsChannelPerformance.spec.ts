// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import AnalyticsChannelPerformance from './AnalyticsChannelPerformance.vue'

const channels = [{
  channel_name: 'Primary', total_requests: 20, success_requests: 19, failed_requests: 1, success_rate: 95,
  total_tokens: 1000, avg_latency_ms: 1200, avg_ttft_ms: 420,
}]

describe('AnalyticsChannelPerformance', () => {
  it('renders channel success and latency metrics', () => {
    const wrapper = mount(AnalyticsChannelPerformance, { props: { channels } })
    expect(wrapper.text()).toContain('上游供应商渠道调度与承载分布')
    expect(wrapper.text()).toContain('Primary')
    expect(wrapper.text()).toContain('95% 成功')
    expect(wrapper.text()).toContain('1.20s')
    expect(wrapper.text()).toContain('0.42s')
  })

  it('does not render the card when there are no channels', () => {
    const wrapper = mount(AnalyticsChannelPerformance, { props: { channels: [] } })
    expect(wrapper.find('.m26-analytics-card').exists()).toBe(false)
  })
})
