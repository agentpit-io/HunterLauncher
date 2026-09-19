import { describe, expect, it } from 'vitest'
import { bytes, duration, percent, thousands } from './format'

describe('千分位', () => {
  it('视觉稿上的数字', () => {
    expect(thousands(300000)).toBe('300,000')
    expect(thousands(42300)).toBe('42,300')
    expect(thousands(0)).toBe('0')
  })
})

describe('字节', () => {
  it('用 SI 单位，与 docker 打印一致', () => {
    expect(bytes(999)).toBe('999 B')
    expect(bytes(1000)).toBe('1.00 kB')
    expect(bytes(152979569)).toBe('153 MB')
    expect(bytes(849179267)).toBe('849 MB')
    expect(bytes(2120000000)).toBe('2.12 GB')
  })

  it('非法输入给「—」而不是 NaN', () => {
    expect(bytes(Number.NaN)).toBe('—')
    expect(bytes(-1)).toBe('—')
  })
})

describe('百分比', () => {
  it('四舍五入并夹在 0–100', () => {
    expect(percent(64, 100)).toBe(64)
    expect(percent(0, 0)).toBe(0)
    expect(percent(200, 100)).toBe(100)
    expect(percent(-5, 100)).toBe(0)
  })
})

describe('时长', () => {
  it('中文', () => {
    expect(duration(45)).toBe('45 秒')
    expect(duration(80)).toBe('1 分 20 秒')
    expect(duration(120)).toBe('2 分')
    expect(duration(3600 * 3 + 60 * 6)).toBe('3 小时 6 分')
    expect(duration(86400 * 3 + 3600 * 6)).toBe('3 天 6 小时')
  })

  it('英文', () => {
    expect(duration(80, 'en')).toBe('1m 20s')
    expect(duration(86400 * 3 + 3600 * 6, 'en')).toBe('3d 6h')
  })

  it('非法输入给「—」', () => {
    expect(duration(Number.NaN)).toBe('—')
  })
})
