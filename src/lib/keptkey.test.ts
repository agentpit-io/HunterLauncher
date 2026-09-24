import { describe, expect, it } from 'vitest'
import { keptKeyExhausted, keptKeyMode } from './keptkey'
import type { KeptKey, KeyCheckResult } from './types'

const check = (over: Partial<KeyCheckResult> = {}): KeyCheckResult => ({
  valid: true,
  reason: 'ok',
  quota: null,
  models: [],
  message: null,
  code: null,
  ...over,
})

const kept = (over: Partial<KeptKey> = {}): KeptKey => ({
  present: true,
  masked: 'hunt_tools_****Mo2',
  path: '<用户目录>/.hunter/app/.env',
  check: check(),
  ...over,
})

describe('I14 · F3 上次保留下来的那把 key', () => {
  it('没有留下来的 key 就照常让用户填', () => {
    expect(keptKeyMode(null)).toBe('ask')
    expect(keptKeyMode(undefined)).toBe('ask')
    expect(keptKeyMode(kept({ present: false, check: null }))).toBe('ask')
    // 还没问回来（check 是 null）时也不能先说「沿用」
    expect(keptKeyMode(kept({ check: null }))).toBe('ask')
  })

  it('验过是好的就直接沿用，不让用户重输', () => {
    expect(keptKeyMode(kept())).toBe('reuse')
  })

  it('验不过时不许说「沿用」，要走「摆出原因」那条路', () => {
    const bad = kept({ check: check({ valid: false, reason: 'rejected', message: '网关不认这把 key' }) })
    expect(keptKeyMode(bad)).toBe('bad')
    expect(keptKeyExhausted(bad)).toBe(false)
  })

  it('额度用完了不算用不了 —— key 本身没问题，照样沿用，但要多说一句', () => {
    const e = kept({
      check: check({ valid: true, reason: 'exhausted', message: '今天的免费额度用完了' }),
    })
    expect(keptKeyMode(e)).toBe('reuse')
    expect(keptKeyExhausted(e)).toBe(true)
    // 好的那一把不该触发这句提醒
    expect(keptKeyExhausted(kept())).toBe(false)
  })
})
