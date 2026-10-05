// Model-name normalization. Upstream providers expose the same underlying
// model under many surface names (vendor prefixes, billing suffixes, date
// stamps, free/preview variants). These pure helpers collapse those into a
// canonical group key so the UI can aggregate equivalent models into one card,
// honoring user-defined custom mappings on top of the built-in rules.
//
// This is a deliberate twin of the normalization in
// internal/channel/canonical.go (CleanModelName, CanonicalModelKey,
// knownEquivalenceGroups, GetCanonicalGroupKey, AreModelsEquivalent,
// SelectCanonicalDisplayName). The two cannot share code — the UI groups model
// cards locally rather than calling the gateway — so a change to the vendor
// prefix list, a billing/date suffix, or an equivalence group here must be
// mirrored there (and vice versa), or the UI will group models differently from
// how the gateway routes them.
//
// Every function is pure: the custom-mapping dictionary is passed in rather
// than read from a reactive ref, which keeps this module free of Vue and
// unit-testable in isolation.

export type ModelMappings = Record<string, string>

const VENDOR_PREFIX_RE = /^(z-ai|qwen|deepseek-ai|deepseek|openai|anthropic|google|mistralai|mistral|meta-llama|meta|nvidia|01-ai|baichuan-inc|minimaxai|minimax|tencent|aliyun|moonshotai|bytedance|togethercomputer)\//i
const BILLING_SUFFIX_RE = /(:free|[-_]free|[-_]trial|[-_]vip|[-_]plus|[-_]preview|[-_]latest|[-_]next)$/i
const DATE_SUFFIX_RE = /(-\d{4}-\d{2}-\d{2}|-\d{8}|-\d{4})$/i
const PUNCT_RE = /[-_.\s/:]+/g

const KNOWN_EQUIVALENCE: string[][] = [
  ['glm53', 'glm53flash'],
  ['glm4', 'glm4flash', 'glm4air'],
  ['deepseekchat', 'deepseekv3'],
  ['deepseekreasoner', 'deepseekr1'],
  ['qwen38flash', 'qwen38flashnext'],
  ['claude35sonnet', 'claude35sonnet20241022'],
  ['claude35haiku', 'claude35haiku20241022'],
  ['claude37sonnet', 'claude37sonnet20250219'],
  ['claude3opus', 'claude3opus20240229'],
  ['gpt4o', 'gpt4o20240806', 'gpt4o20240513'],
  ['gpt4omini', 'gpt4omini20240718'],
]

// cleanModelName strips a leading vendor prefix and trailing billing/date
// suffixes. A `gpt`/`claude`/`gemini` prefix is preserved (e.g. an OpenRouter
// `openai/gpt-4o` keeps `gpt-4o`, but `openai/` is removed by the vendor list),
// because those tokens are part of the recognizable model identity.
export function cleanModelName(raw: string): string {
  let s = (raw || '').trim()
  if (!s) return ''
  if (VENDOR_PREFIX_RE.test(s)) {
    s = s.replace(VENDOR_PREFIX_RE, '')
  } else {
    const idx = s.indexOf('/')
    if (idx !== -1 && idx < s.length - 1) {
      const prefix = s.slice(0, idx).toLowerCase()
      if (!prefix.includes('gpt') && !prefix.includes('claude') && !prefix.includes('gemini')) {
        s = s.slice(idx + 1)
      }
    }
  }
  s = s.replace(BILLING_SUFFIX_RE, '')
  s = s.replace(DATE_SUFFIX_RE, '')
  return s.trim()
}

// canonicalModelKey reduces a name to lowercase alphanumerics for equivalence
// comparison, so punctuation and casing differences don't split a group.
export function canonicalModelKey(raw: string): string {
  const cleaned = cleanModelName(raw)
  return cleaned.toLowerCase().replace(PUNCT_RE, '')
}

// getCanonicalGroupKey resolves a raw model name to the key its aggregation
// card should use. It chases custom mappings (up to 5 hops to avoid cycles),
// falling back to cleaned-name and canonical-key lookups, then collapses known
// equivalence groups to their first member.
export function getCanonicalGroupKey(raw: string, mappings: ModelMappings): string {
  const trim = (raw || '').trim()
  if (!trim) return ''
  let curr = trim
  for (let i = 0; i < 5; i++) {
    let target = mappings[curr] || mappings[curr.toLowerCase()]
    if (!target) {
      const cleaned = cleanModelName(curr).toLowerCase()
      if (cleaned && cleaned !== curr.toLowerCase()) {
        target = mappings[cleaned]
      }
    }
    if (!target) {
      const cKey = canonicalModelKey(curr)
      if (cKey) {
        for (const [k, v] of Object.entries(mappings)) {
          if (canonicalModelKey(k) === cKey && v) {
            target = v
            break
          }
        }
      }
    }
    if (target && target.toLowerCase() !== curr.toLowerCase()) {
      curr = target
    } else {
      break
    }
  }
  const key = canonicalModelKey(curr)
  if (!key) return curr
  for (const group of KNOWN_EQUIVALENCE) {
    if (group.includes(key)) {
      return group[0]
    }
  }
  return key
}

// isModelEquivalent reports whether two raw names resolve to the same group.
export function isModelEquivalent(a: string, b: string, mappings: ModelMappings): boolean {
  if (a.toLowerCase() === b.toLowerCase()) return true
  const keyA = getCanonicalGroupKey(a, mappings)
  const keyB = getCanonicalGroupKey(b, mappings)
  return Boolean(keyA && keyB && keyA === keyB)
}

// selectCanonicalDisplayName picks the most user-friendly name from a set of
// equivalent raw names: a custom-mapping target wins, otherwise the shortest,
// prefix-free, non-free/preview/dated variant scores highest.
export function selectCanonicalDisplayName(models: string[], mappings: ModelMappings): string {
  if (!models.length) return ''
  let customTarget = ''
  for (const m of models) {
    const t = mappings[m] || mappings[m.toLowerCase()]
    if (t) {
      customTarget = t
      break
    }
  }
  const candidateList = [...models]
  if (customTarget && !candidateList.some(c => c.toLowerCase() === customTarget.toLowerCase())) {
    candidateList.push(customTarget)
  }

  let best = ''
  let bestScore = -999
  for (const m of candidateList) {
    let score = 0
    const lower = m.toLowerCase()
    if (customTarget && lower === customTarget.toLowerCase()) score += 60
    if (m.includes('/')) score -= 30
    if (lower.includes('-free') || lower.includes(':free') || lower.includes('_free')) score -= 40
    if (lower.includes('-preview') || lower.includes('-trial') || lower.includes('-next')) score -= 20
    if (DATE_SUFFIX_RE.test(lower)) score -= 15
    if (lower.includes('-flash') || lower.includes('-pro') || lower.includes('-turbo')) score += 10
    score -= m.length
    const isLower = m === lower
    const bestIsLower = best === best.toLowerCase()
    if (score > bestScore || !best || (score === bestScore && isLower && !bestIsLower)) {
      bestScore = score
      best = m
    }
  }
  if (best) {
    if (customTarget && best.toLowerCase() === customTarget.toLowerCase()) {
      return best
    }
    const cleaned = cleanModelName(best)
    if (cleaned && !cleaned.includes('/') && !cleaned.toLowerCase().includes('free')) {
      return cleaned
    }
    return best
  }
  return models[0]
}
