/*
 * 选盘卡片（R5 · U-03）里的**纯逻辑**。
 * ---------------------------------------------------------------------------
 * 组件那一层只负责画，判断都在这里 —— 没有 DOM、没有 React，可以直接单测。
 *
 * ## 一条硬规矩：界面不产生数字
 *
 * 这个文件里**没有任何一个容量常量**。所有容量都从 `DiskPlan` 来，
 * 而 `DiskPlan` 是 Rust 当场探的（卷的容量与剩余）或者代码里的常量
 * （第一次装的那三笔、建议阈值）。这里只做三件事：换算、挑、拼句子。
 *
 * ## 另一条：上限与实占**永远是两句**
 *
 * I14·F2 把稀疏文件报成「86.1 GB」而实际只占 5.8 GB（差 15 倍），
 * 所以这两个数在类型上就是两个字段（`capGb` / `usedGb`），
 * 到了界面上也必须落在两行里 —— 见 `capText` 与 `usedText`。
 */

import type { DiskPlan, DiskVolume } from './types'

const GIB = 1024 ** 3

/** `chosen` 指向的那块盘。找不到就是 `null` —— 界面必须能处理，不许瞎选一块顶上。 */
export function chosenVolume(plan: DiskPlan): DiskVolume | null {
  if (plan.chosen === null) return null
  return plan.volumes.find((v) => v.mount === plan.chosen) ?? null
}

/** 第一块被标了系统盘的卷（解释「为什么不是它」要用）。 */
export function systemVolume(plan: DiskPlan): DiskVolume | null {
  return plan.volumes.find((v) => v.isSystem) ?? null
}

/** 非系统盘里剩余空间最大的那一块。 */
export function largestNonSystem(plan: DiskPlan): DiskVolume | null {
  let best: DiskVolume | null = null
  for (const v of plan.volumes) {
    if (v.isSystem) continue
    if (best === null || v.freeBytes > best.freeBytes) best = v
  }
  return best
}

/** 一块盘还剩多少 GB（GiB，向下取整 —— 少报一点，不把话说满）。 */
export function freeGb(v: DiskVolume): number {
  return Math.floor(v.freeBytes / GIB)
}

/** 字节 → 「约 X GB」（一位小数）。界面上的明细用这个。 */
export function approxGb(bytes: number): string {
  const gb = bytes / GIB
  return gb < 0.1 ? '<0.1' : gb.toFixed(1)
}

/**
 * **全不合格**（`chosen === null`）时，告诉用户还差多少 —— 这是 U-03
 * 明确要求的那一句「不许静默失败或瞎选一个」。
 *
 * 返回 `null` 表示**不是「差一点」，是一块候选盘都没有**：
 * 枚举结果里只有系统盘（别的盘要么太小、要么不可写、要么是 U 盘）。
 * 那两种情况要说的话不一样，所以这里分成两种返回值。
 */
export function shortfall(
  plan: DiskPlan,
): { mount: string; freeGb: number; needGb: number; shortGb: number } | null {
  const best = largestNonSystem(plan)
  if (!best) return null
  const free = freeGb(best)
  if (free >= plan.needGb) return null // 有余量却还是没选出来 —— 不是「差空间」，别乱解释
  return { mount: best.mount, freeGb: free, needGb: plan.needGb, shortGb: plan.needGb - free }
}

/**
 * 运行时磁盘**上限**那一行（第一句）。
 *
 * `null` 是「量不到」而不是 0 —— 两者在界面上必须是两句不同的话。
 */
export function capText(plan: DiskPlan): string | null {
  return plan.capGb === null ? null : String(plan.capGb)
}

/**
 * **实际已占**那一行（第二句）。
 *
 * 三种情形分开说，一句都不许合并：
 *   · 量到了 → `{ gb }`；
 *   · 没量到，而且内置运行时压根没装（`usedGb === null` 且 `dataDir` 里头还没有东西）
 *     → `'none'`（「还没装，所以是 0」）；
 *   · 装了但问不出来 → `'unknown'`（「量不到」）。
 *
 * 这里判「装没装」用的是**量不到 + 盘上已经有 Hunter 数据**这个组合 ——
 * 见 `hasHunterAnywhere`。不猜、不编。
 */
export type UsedState = { kind: 'gb'; gb: number } | { kind: 'none' } | { kind: 'unknown' }

export function usedState(plan: DiskPlan): UsedState {
  if (plan.usedGb !== null) return { kind: 'gb', gb: plan.usedGb }
  return hasHunterAnywhere(plan) ? { kind: 'unknown' } : { kind: 'none' }
}

/** 有没有哪块盘上已经躺着 Hunter 的运行时（Rust 侧 `hasHunter`，当场比的挂载点）。 */
export function hasHunterAnywhere(plan: DiskPlan): boolean {
  return plan.volumes.some((v) => v.hasHunter)
}

/** 「现在装在这」那一行显示哪个路径：配置里有就用配置的，没有就是实际落地路径。 */
export function currentDir(plan: DiskPlan): string {
  const custom = plan.dataDir.trim()
  return custom === '' ? plan.runtimeDir : custom
}

/** 选盘卡片要不要显示「换一块盘」那一节：盘少于两块时没得选，不显示。 */
export function canChangeDisk(plan: DiskPlan): boolean {
  return plan.volumes.filter((v) => !v.isSystem).length > 0
}
