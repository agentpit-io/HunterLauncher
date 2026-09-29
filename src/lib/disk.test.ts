import { describe, expect, it } from 'vitest'
import { demoDiskPlan, demoDiskPlanNone } from './demo'
import {
  approxGb,
  canChangeDisk,
  capText,
  chosenVolume,
  currentDir,
  freeGb,
  hasHunterAnywhere,
  largestNonSystem,
  shortfall,
  systemVolume,
  usedState,
} from './disk'
import type { DiskPlan } from './types'

const GIB = 1024 ** 3

function plan(over: Partial<DiskPlan> = {}): DiskPlan {
  return { ...demoDiskPlan, ...over }
}

describe('R5 · 选盘卡片：挑盘', () => {
  it('选中盘按 chosen 去列表里找，找得到就是它', () => {
    expect(chosenVolume(plan())?.mount).toBe('D:\\')
  })

  it('chosen 指的是列表里没有的盘 → null，不瞎选一块顶上', () => {
    expect(chosenVolume(plan({ chosen: 'Z:\\' }))).toBeNull()
  })

  it('chosen 为 null（全不合格）→ null', () => {
    expect(chosenVolume(demoDiskPlanNone)).toBeNull()
  })

  it('系统盘与非系统盘分得开', () => {
    expect(systemVolume(plan())?.mount).toBe('C:\\')
    expect(largestNonSystem(plan())?.mount).toBe('D:\\')
  })

  it('只有系统盘时，非系统盘那一个也是 null', () => {
    expect(largestNonSystem(demoDiskPlanNone)).toBeNull()
    expect(canChangeDisk(demoDiskPlanNone)).toBe(false)
  })

  it('剩余空间按 GiB 向下取整（少报一点，不把话说满）', () => {
    const v = { mount: 'D:\\', totalBytes: 100 * GIB, freeBytes: 212.9 * GIB, isSystem: false, hasHunter: false }
    expect(freeGb(v)).toBe(212)
  })
})

describe('R5 · 选盘卡片：上限与实占是两句', () => {
  it('量到上限时给出数字；量不到时是 null（不是 0）', () => {
    expect(capText(plan({ capGb: 60 }))).toBe('60')
    expect(capText(plan({ capGb: null }))).toBeNull()
  })

  it('量到实占 → 报那个数', () => {
    expect(usedState(plan({ usedGb: 5 }))).toEqual({ kind: 'gb', gb: 5 })
  })

  it('没装过 → 「还没有装，所以是 0」，不是「量不到」', () => {
    // 盘上没有任何一块有 Hunter 的数据 = 这台机器还没装过
    expect(usedState(plan({ usedGb: null }))).toEqual({ kind: 'none' })
  })

  it('装了但问不出来 → 「量不到」—— 绝不拿宿主上的文件大小顶替（I14·F2 那 15 倍）', () => {
    const installed = plan({
      usedGb: null,
      volumes: [
        { mount: 'C:\\', totalBytes: 512 * GIB, freeBytes: 18 * GIB, isSystem: true, hasHunter: false },
        { mount: 'D:\\', totalBytes: 1024 * GIB, freeBytes: 212 * GIB, isSystem: false, hasHunter: true },
      ],
    })
    expect(hasHunterAnywhere(installed)).toBe(true)
    expect(usedState(installed)).toEqual({ kind: 'unknown' })
  })

  it('上限与实占**不可能**被拼成同一个字符串（类型上就是两条路）', () => {
    const p = plan({ capGb: 60, usedGb: 5 })
    expect(capText(p)).toBe('60')
    expect(usedState(p)).toEqual({ kind: 'gb', gb: 5 })
  })
})

describe('R5 · 选盘卡片：全不合格时必须有话可说', () => {
  it('有非系统盘但空间不够 → 算出还差多少', () => {
    const p = plan({
      chosen: null,
      needGb: 15,
      volumes: [
        { mount: 'C:\\', totalBytes: 512 * GIB, freeBytes: 4 * GIB, isSystem: true, hasHunter: false },
        { mount: 'D:\\', totalBytes: 200 * GIB, freeBytes: 6 * GIB, isSystem: false, hasHunter: false },
      ],
    })
    expect(shortfall(p)).toEqual({ mount: 'D:\\', freeGb: 6, needGb: 15, shortGb: 9 })
  })

  it('一块非系统盘都没有 → null（那是「换块盘」而不是「腾空间」，两句话不一样）', () => {
    expect(shortfall(demoDiskPlanNone)).toBeNull()
  })

  it('空间其实够却没选出来 → 不硬解释成「差空间」', () => {
    const p = plan({
      chosen: null,
      volumes: [
        { mount: 'D:\\', totalBytes: 1024 * GIB, freeBytes: 900 * GIB, isSystem: false, hasHunter: false },
      ],
    })
    expect(shortfall(p)).toBeNull()
  })
})

describe('R5 · 选盘卡片：其余几行', () => {
  it('「现在装在这」：配置里有就用配置的，没有就是实际落地路径', () => {
    expect(currentDir(plan({ dataDir: 'D:\\Hunter' }))).toBe('D:\\Hunter')
    expect(currentDir(plan({ dataDir: '  ' }))).toBe(plan().runtimeDir)
  })

  it('字节换成人看的「约 X GB」，一位小数', () => {
    expect(approxGb(420 * 1024 * 1024)).toBe('0.4')
    expect(approxGb(3_900 * 1024 * 1024)).toBe('3.8')
    expect(approxGb(1_700 * 1024 * 1024)).toBe('1.7')
    // 小于 0.1 的不显示成 0.0（那会看起来像「不占地方」）
    expect(approxGb(1024)).toBe('<0.1')
  })

  it('三笔明细加起来 ≈ 「第一次装大约 6 GB」（Rust 那边算好的那个数）', () => {
    const sum = plan().parts.reduce((a, p) => a + Number(approxGb(p.bytes)), 0)
    expect(plan().firstInstallGb).toBe(6)
    expect(sum).toBeGreaterThan(5.5)
    expect(sum).toBeLessThanOrEqual(6.5)
  })

  it('只要还有一块非系统盘，「换一块盘」就该出现', () => {
    expect(canChangeDisk(plan())).toBe(true)
  })
})
