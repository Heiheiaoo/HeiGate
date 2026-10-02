// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import AnalyticsModelBreakdown from './AnalyticsModelBreakdown.vue'

const models = [{
  model: 'gpt-5', total_requests: 20, success_requests: 19, failed_requests: 1, success_rate: 95,
  total_tokens: 12500, prompt_tokens: 10000, completion_tokens: 2500,
  avg_latency_ms: 1200, min_latency_ms: 300, max_latency_ms: 5000, avg_ttft_ms: 420, top_channel: 'Primary',
}]

describe('AnalyticsModelBreakdown', () => {
  it('renders model usage and latency details', () => {
    const wrapper = mount(AnalyticsModelBreakdown, { props: { models, totalRequests: 40, search: '' } })
    expect(wrapper.text()).toContain('gpt-5')
    expect(wrapper.text()).toContain('Primary')
    expect(wrapper.text()).toContain('12.5K')
    expect(wrapper.text()).toContain('1.20s')
    expect(wrapper.text()).toContain('50.0%')
  })

  it('emits search updates and renders empty state', async () => {
    const wrapper = mount(AnalyticsModelBreakdown, { props: { models: [], totalRequests: 0, search: '' } })
    await wrapper.get('input').setValue('claude')
    expect(wrapper.emitted('update:search')).toEqual([['claude']])
    expect(wrapper.text()).toContain('无匹配的模型记录')
  })
})
