export async function configRequest(path: string, body?: unknown) {
  const response = await fetch('/api/' + path, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'Content-Type': 'application/json', 'X-HeiGate-Request': '1' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30000),
  })
  const result = await response.json()
  if (!response.ok) throw new Error(result.error || '配置操作失败')
  return result.data
}

export function downloadConfig(payload: unknown, containsSecrets: boolean) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' }))
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `heigate-${containsSecrets ? 'secret-backup' : 'redacted-config'}-${new Date().toISOString().slice(0, 10)}.json`
  anchor.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
