import { describe, expect, it } from 'vitest'
import {
  cleanModelName,
  canonicalModelKey,
  getCanonicalGroupKey,
  isModelEquivalent,
  selectCanonicalDisplayName,
  type ModelMappings,
} from '@/services/modelNaming'

const NO_MAPPINGS: ModelMappings = {}

describe('cleanModelName', () => {
  it('strips a known vendor prefix', () => {
    expect(cleanModelName('deepseek-ai/DeepSeek-V3')).toBe('DeepSeek-V3')
    expect(cleanModelName('qwen/Qwen2.5-72B')).toBe('Qwen2.5-72B')
  })

  it('strips an unknown vendor prefix that is not gpt/claude/gemini', () => {
    expect(cleanModelName('someorg/cool-model')).toBe('cool-model')
  })

  it('preserves a prefix that names gpt/claude/gemini', () => {
    // The prefix is only kept when it is NOT in the vendor list; `openai/` is a
    // vendor prefix so it is stripped, leaving the identity token.
    expect(cleanModelName('openai/gpt-4o')).toBe('gpt-4o')
    // A non-vendor prefix that mentions gpt/claude/gemini is left intact.
    expect(cleanModelName('azure-gpt/gpt-4o')).toBe('azure-gpt/gpt-4o')
  })

  it('strips billing and date suffixes', () => {
    expect(cleanModelName('gpt-4o:free')).toBe('gpt-4o')
    expect(cleanModelName('deepseek-chat-free')).toBe('deepseek-chat')
    expect(cleanModelName('claude-3-opus-20240229')).toBe('claude-3-opus')
    expect(cleanModelName('some-model-2025')).toBe('some-model')
  })

  it('returns empty string for blank input', () => {
    expect(cleanModelName('')).toBe('')
    expect(cleanModelName('   ')).toBe('')
  })
})

describe('canonicalModelKey', () => {
  it('lowercases and removes punctuation', () => {
    expect(canonicalModelKey('DeepSeek-V3')).toBe('deepseekv3')
    expect(canonicalModelKey('GPT-4o')).toBe('gpt4o')
    expect(canonicalModelKey('claude.3.opus')).toBe('claude3opus')
  })
})

describe('getCanonicalGroupKey', () => {
  it('collapses a known equivalence group to its first member', () => {
    expect(getCanonicalGroupKey('deepseek-v3', NO_MAPPINGS)).toBe('deepseekchat')
    expect(getCanonicalGroupKey('deepseek-chat', NO_MAPPINGS)).toBe('deepseekchat')
    expect(getCanonicalGroupKey('gpt-4o-2024-08-06', NO_MAPPINGS)).toBe('gpt4o')
  })

  it('follows a custom mapping to its target', () => {
    const mappings: ModelMappings = { 'my-alias': 'gpt-4o' }
    expect(getCanonicalGroupKey('my-alias', mappings)).toBe('gpt4o')
  })

  it('chases a chain of custom mappings without looping forever', () => {
    const mappings: ModelMappings = { a: 'b', b: 'c', c: 'gpt-4o' }
    expect(getCanonicalGroupKey('a', mappings)).toBe('gpt4o')
  })

  it('stops at the 5-hop limit on a cyclic mapping', () => {
    const mappings: ModelMappings = { a: 'b', b: 'a' }
    // Must terminate and return a stable key rather than hang.
    const key = getCanonicalGroupKey('a', mappings)
    expect(typeof key).toBe('string')
    expect(key.length).toBeGreaterThan(0)
  })

  it('falls back to the canonical key for an unknown model', () => {
    expect(getCanonicalGroupKey('brand-new-model', NO_MAPPINGS)).toBe('brandnewmodel')
  })
})

describe('isModelEquivalent', () => {
  it('is true for identical names ignoring case', () => {
    expect(isModelEquivalent('GPT-4o', 'gpt-4o', NO_MAPPINGS)).toBe(true)
  })

  it('is true across a known equivalence group', () => {
    expect(isModelEquivalent('deepseek-chat', 'deepseek-v3', NO_MAPPINGS)).toBe(true)
  })

  it('is transitive through a custom mapping', () => {
    const mappings: ModelMappings = { 'vendor/foo-model': 'gpt-4o' }
    expect(isModelEquivalent('vendor/foo-model', 'gpt-4o-2024-08-06', mappings)).toBe(true)
  })

  it('is false for genuinely different models', () => {
    expect(isModelEquivalent('gpt-4o', 'claude-3-opus', NO_MAPPINGS)).toBe(false)
  })
})

describe('selectCanonicalDisplayName', () => {
  it('returns empty string for an empty list', () => {
    expect(selectCanonicalDisplayName([], NO_MAPPINGS)).toBe('')
  })

  it('prefers a clean short name over a prefixed or free variant', () => {
    const picked = selectCanonicalDisplayName(
      ['openrouter/deepseek-v3:free', 'deepseek-v3'],
      NO_MAPPINGS,
    )
    expect(picked).toBe('deepseek-v3')
  })

  it('penalizes dated and free variants', () => {
    const picked = selectCanonicalDisplayName(
      ['gpt-4o-2024-08-06', 'gpt-4o', 'gpt-4o-free'],
      NO_MAPPINGS,
    )
    expect(picked).toBe('gpt-4o')
  })

  it('honors a custom-mapping target as the display name', () => {
    const mappings: ModelMappings = { 'raw-name': 'Preferred Name' }
    const picked = selectCanonicalDisplayName(['raw-name'], mappings)
    expect(picked).toBe('Preferred Name')
  })
})
