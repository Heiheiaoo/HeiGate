// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import AnalyticsTrendCharts from './AnalyticsTrendCharts.vue'

const analyticsData = {
  days: 7,
  summary: { total_requests: 3, success_requests: 2, failed_requests: 1, success_rate: 66.7, total_tokens: 1200, prompt_tokens: 900, completion_tokens: 300, avg_latency_ms: 900, avg_ttft_ms: 300 },
  daily: [{ date: '2026-10-03', total_requests: 3, success_requests: 2, failed_requests: 1, total_tokens: 1200, prompt_tokens: 900, completion_tokens: 300, avg_latency_ms: 900, avg_ttft_ms: 300 }],
  models: [],
  channels: [],
}

describe('AnalyticsTrendCharts', () => {
  it('renders both trend charts and tooltip values', () => {
    const wrapper = mount(AnalyticsTrendCharts, { props: { analyticsData, maxDailyRequests: 3, maxDailyTokens: 1200 } })
    expect(wrapper.text()).toContain('每日请求量走势')
    expect(wrapper.text()).toContain('每日 Token 消耗走势')
    expect(wrapper.text()).toContain('2026-10-03')
    expect(wrapper.text()).toContain('1.2K')
    expect(wrapper.findAll('.m26-bar-column-group')).toHaveLength(2)
  })

  it('shows empty state when there are no daily records', () => {
    const wrapper = mount(AnalyticsTrendCharts, { props: { analyticsData: null, maxDailyRequests: 1, maxDailyTokens: 1 } })
    expect(wrapper.findAll('.m26-analytics-empty')).toHaveLength(2)
    expect(wrapper.text()).toContain('所选时间范围内暂无调用记录')
  })
})
