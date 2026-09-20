import { describe, expect, it } from 'vitest'
import { bytes, duration, isoToShanghai, percent, shanghaiStamp, thousands } from './format'

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

describe('shanghaiStamp', () => {
  it('网关的 ISO 砍成「日期 时分（上海）」', () => {
    // 这一条是 I3 回归里从网关真实拿到的 reset_at
    expect(shanghaiStamp('2026-09-21T00:00:00+08:00')).toBe('2026-09-21 00:00（上海）')
  })
  it('不做时区换算 —— 偏移量不一样也只砍格式', () => {
    expect(shanghaiStamp('2026-09-21T00:00:00Z')).toBe('2026-09-21 00:00（上海）')
  })
  it('认不出来的一律 null，不猜一个时间出来', () => {
    expect(shanghaiStamp(null)).toBeNull()
    expect(shanghaiStamp(undefined)).toBeNull()
    expect(shanghaiStamp('')).toBeNull()
    expect(shanghaiStamp('明天零点')).toBeNull()
    expect(shanghaiStamp('2026-09-21')).toBeNull()
    expect(shanghaiStamp('2026-9-21T00:00:00+08:00')).toBeNull()
    expect(shanghaiStamp('2026-09-21T0:00:00+08:00')).toBeNull()
  })
})

describe('isoToShanghai', () => {
  it('UTC 的发布时间换算成上海时间（总控规则：时间一律上海）', () => {
    // 这一条取自 I3 回归截图里更新页真实显示过的那个值（原来是原样上屏的 …Z）
    expect(isoToShanghai('2026-09-19T09:47:29Z')).toBe('2026-09-19 17:47（上海）')
  })
  it('跨日也对', () => {
    expect(isoToShanghai('2026-09-19T23:30:00Z')).toBe('2026-09-20 07:30（上海）')
  })
  it('已经带 +08:00 的不会再加一次八小时', () => {
    expect(isoToShanghai('2026-09-21T00:00:00+08:00')).toBe('2026-09-21 00:00（上海）')
  })
  it('解析不出来就原样返回，不猜', () => {
    expect(isoToShanghai('刚刚')).toBe('刚刚')
    expect(isoToShanghai('')).toBe('')
  })
})
