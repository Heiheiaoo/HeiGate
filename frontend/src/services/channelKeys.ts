export type ChannelKey = {
  id: number
  channel_id: number
  name: string
  masked_secret: string
  enabled: boolean
  position: number
  auth_invalid: boolean
  cooldown_until: string | null
  last_error: string
  attempts: number
  successes: number
  rate_limits: number
  prompt_tokens: number
  completion_tokens: number
}

export async function keyRequest(channelId: number, method = 'GET', body?: unknown, keyId?: number): Promise<ChannelKey[]> {
  const response = await fetch(`/api/channels/${channelId}/keys${keyId === undefined ? '' : '/' + keyId}`, {
    method,
    headers: { 'Content-Type': 'application/json', 'X-HeiGate-Request': '1' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30000),
  })
  const result = await response.json()
  if (!response.ok) throw new Error(result.error || 'Key 操作失败')
  return result.data
}
