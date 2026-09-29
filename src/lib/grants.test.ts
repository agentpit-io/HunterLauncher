import { describe, expect, it } from 'vitest'
import { demoAudit } from './demo'
import {
  grantsFromConfig,
  grantsToConfig,
  hasGrant,
  lastGrantChange,
  normalize,
  parseAudit,
  parseAuditLine,
  toggleGrant,
} from './grants'

describe('R5 · 三级授权：勾是一个前缀', () => {
  it('后端给的默认是「一级 + 二级」，界面照读', () => {
    expect(grantsFromConfig(['l1', 'l2'])).toEqual(['l1', 'l2'])
    expect(hasGrant(grantsFromConfig(['l1', 'l2']), 'l3')).toBe(false)
  })

  it('一份也没有 = 一档都没授权', () => {
    expect(grantsFromConfig([])).toEqual([])
    expect(grantsFromConfig(null)).toEqual([])
  })

  it('配置里只写了高档也按「含低档」理解（Rust 存的是上限）', () => {
    expect(grantsFromConfig(['l3'])).toEqual(['l1', 'l2', 'l3'])
    expect(grantsFromConfig(['l2'])).toEqual(['l1', 'l2'])
  })

  it('手改配置时写的 1/2/3 与枚举序列化名也认', () => {
    expect(grantsFromConfig(['1', '2'])).toEqual(['l1', 'l2'])
    expect(grantsFromConfig(['elevated'])).toEqual(['l1', 'l2', 'l3'])
    expect(grantsFromConfig(['L3'])).toEqual(['l1', 'l2', 'l3'])
  })

  it('认不得的值忽略掉 —— 宁可少授权，与 Rust 同一条', () => {
    expect(grantsFromConfig(['l2', 'l9', '随便什么'])).toEqual(['l1', 'l2'])
  })

  it('规范化永远回到前缀', () => {
    expect(normalize(['l3', 'l1'])).toEqual(['l1', 'l2', 'l3'])
    expect(normalize(['l2', 'l3'])).toEqual(['l1', 'l2', 'l3'])
    expect(normalize([])).toEqual([])
  })
})

describe('R5 · 三级授权：勾与取消', () => {
  it('勾三级 → 一、二、三级都算上', () => {
    expect(toggleGrant(['l1', 'l2'], 'l3', true)).toEqual(['l1', 'l2', 'l3'])
  })

  it('取消三级 → 回到二级（不是一档都没有）', () => {
    expect(toggleGrant(['l1', 'l2', 'l3'], 'l3', false)).toEqual(['l1', 'l2'])
  })

  it('取消二级 → 回到只授权一级', () => {
    expect(toggleGrant(['l1', 'l2'], 'l2', false)).toEqual(['l1'])
  })

  it('取消一级 → 一档都不授权（二、三级都是它的超集，站不住）', () => {
    expect(toggleGrant(['l1', 'l2', 'l3'], 'l1', false)).toEqual([])
  })

  it('取消再勾回来，得到的还是同一个前缀', () => {
    const off = toggleGrant(['l1', 'l2'], 'l3', false)
    expect(toggleGrant(off, 'l3', true)).toEqual(['l1', 'l2', 'l3'])
  })

  it('写回配置的是字符串表，与 Rust 的 Grant::as_str 逐字一致', () => {
    expect(grantsToConfig(['l1', 'l2'])).toEqual(['l1', 'l2'])
    expect(grantsToConfig([])).toEqual([])
    expect(grantsToConfig(['l3', 'l1'])).toEqual(['l1', 'l2', 'l3'])
  })
})

describe('R5 · 授权审计：什么时候改成过什么', () => {
  it('解得出时间、谁做的、结果那一句', () => {
    const e = parseAuditLine(demoAudit[0]!)
    expect(e).not.toBeNull()
    expect(e!.at).toBe('2026-09-21 10:30:00')
    expect(e!.by).toBe('user')
    expect(e!.result).toContain('授权档位')
    // 授权页那条 args 里没有 grants，所以它**不算改了授权档次**
    expect(e!.isGrantChange).toBe(false)
  })

  it('带 grants 的那一条才算「改了授权」，并解出是哪几档', () => {
    const e = parseAuditLine(demoAudit[3]!)
    expect(e!.isGrantChange).toBe(true)
    expect(e!.grants).toEqual(['l1', 'l2', 'l3'])
    expect(e!.at).toBe('2026-09-21 11:02:44')
  })

  it('不是 consent 的动作不算改了授权（哪怕 args 里恰好有 grants）', () => {
    const line = JSON.stringify({ at: 'x', action: 'remap_ports', args: { grants: 'l1' }, by: 'rule', result: 'y' })
    expect(parseAuditLine(line)!.isGrantChange).toBe(false)
  })

  it('解不开的行不丢 —— 原样留在 raw 里，界面照着显示', () => {
    const { entries, raw } = parseAudit(['不是 JSON', '', demoAudit[0]!])
    expect(entries).toHaveLength(1)
    expect(raw).toEqual(['不是 JSON'])
  })

  it('空行直接跳过，不进 raw（那不是「解不开」，是没有东西）', () => {
    expect(parseAudit(['', '   ']).raw).toEqual([])
  })

  it('拿得到最近一次改授权', () => {
    const { entries } = parseAudit(demoAudit)
    const last = lastGrantChange(entries)
    expect(last?.at).toBe('2026-09-21 11:02:44')
  })

  it('一次都没改过授权 → null（界面不许编一个时间出来）', () => {
    const { entries } = parseAudit([demoAudit[1]!])
    expect(lastGrantChange(entries)).toBeNull()
  })
})
