/**
 * I18 · U5：升级过程那块进度条**不许编数字**（方案 §3.3）。
 *
 * 每一条对应一条红线，反例都写成「真的会犯的那个错」：
 * 拿 `downloadedBytes` 当「本次已下载」、拿 0% 顶替「还没算出来」、
 * 用「已用时 ÷ 比例」凑一个秒数。这几件事在真机上看起来都「挺像那么回事」，
 * 所以只能靠断言拦。
 */
import { describe, expect, it } from 'vitest'
import { pullImageBytes, pullStateText, pullView, type PullTexts } from './pull'
import type { PullProgress } from './types'

/** 与 `zh-CN.ts` 的 `pull:` 命名空间逐字对应（改文案时这里要跟着改）。 */
const t: PullTexts = {
  sizeLine: (done, total) => `${done} / ${total}`,
  sizeComputing: '正在算…',
  downloaded: (v) => `本次已下载 ${v}`,
  noDownload: '这些镜像本机都已有，没有下载',
  speedLine: (v) => `当前速度 ${v}/秒`,
  eta: (x) => `预计还需 ${x}`,
  etaUnknown: '剩余时间未知',
  percentUnit: '%',
  retrying: (n) => `第 ${n} 次尝试（上一次失败后已自动换源）`,
  stateDownloading: '下载中',
  stateExtracting: '解压中',
  stateDone: '完成',
  stateFailed: '失败',
  statePending: '等待',
}

function progress(over: Partial<PullProgress> = {}): PullProgress {
  return {
    phase: 'pulling',
    registry: 'ghcr.io/agentpit-io',
    registryLabel: 'GHCR · GitHub',
    images: [],
    totalBytes: 748_000_000,
    downloadedBytes: 132_000_000,
    netBytes: 28_000_000,
    percent: 18,
    speedBps: 4_800_000,
    etaSeconds: 80,
    attempt: 1,
    log: [],
    error: null,
    errorCode: null,
    ...over,
  }
}

describe('U5 · 升级进度那一块显示什么', () => {
  it('总量没算出来时显示「正在算…」，**不许显示 0%**', () => {
    // manifest 还没读完：`totalBytes` 是 0，而后端给的 `percent` 也是 0
    const v = pullView(progress({ totalBytes: 0, percent: 0 }), t)
    expect(v.size).toBe('正在算…')
    expect(v.percent).toBeNull()
    expect(v.percent ?? '').not.toContain('0')
  })

  it('「本次已下载」用的是 netBytes，不是 downloadedBytes', () => {
    // 本机已经有 104 MB 的层 —— 那部分 `downloadedBytes` 算得进去，
    // 而它一个字节都没过网（`compose.rs` 的 `net_bytes` 注释与 I16 · P2-5 都记着这件事）
    const p = progress({ downloadedBytes: 132_000_000, netBytes: 28_000_000 })
    const v = pullView(p, t)
    expect(v.net).toBe('本次已下载 28 MB')
    expect(v.net).not.toContain('132')
    // 进度条那一行仍然用 downloadedBytes —— 用户关心的是整体完成度，那是另一件事
    expect(v.size).toBe('132 MB / 748 MB')
  })

  it('etaSeconds 为 null 时说「剩余时间未知」，**不出现任何编出来的秒数**', () => {
    const v = pullView(progress({ etaSeconds: null, speedBps: null }), t)
    expect(v.eta).toBe('剩余时间未知')
    // 「已用时 ÷ 已完成比例」那类估算会在这里留下一个数字 —— 一个都不许有
    expect(v.eta).not.toMatch(/\d/)
    // 速度同样：没样本就不显示，不编
    expect(v.speed).toBeNull()
  })

  it('样本够了才显示速度与预计剩余', () => {
    const v = pullView(progress({ speedBps: 4_800_000, etaSeconds: 80 }), t)
    expect(v.speed).toBe('当前速度 4.80 MB/秒')
    expect(v.eta).toBe('预计还需 1 分 20 秒')
  })

  it('本机全都有时说「没有下载」，**不许出现「已下载 X」**', () => {
    // 客户那台 Windows 就是这样：六个镜像本机都有，`netBytes` 是 0，
    // 而 `downloadedBytes` 是满的。拿后者报数会说出「已下载 748 MB，用时 1 秒」
    const v = pullView(
      progress({ phase: 'done', netBytes: 0, downloadedBytes: 748_000_000, percent: 100 }),
      t,
    )
    expect(v.nothing).toBe(true)
    expect(v.size).toBe('这些镜像本机都已有，没有下载')
    expect(v.net).toBeNull()
    expect(v.percent).toBeNull()
  })

  it('拉完了就不再显示「预计剩余」', () => {
    const v = pullView(progress({ phase: 'done', netBytes: 28_000_000 }), t)
    expect(v.eta).toBeNull()
  })

  it('换源重试要说出来，第几次一眼看得见', () => {
    expect(pullView(progress({ attempt: 1 }), t).attempt).toBeNull()
    expect(pullView(progress({ attempt: 2 }), t).attempt).toBe(
      '第 2 次尝试（上一次失败后已自动换源）',
    )
  })

  it('速度是 0 时也不显示（还没动起来，不是「每秒 0 字节」）', () => {
    expect(pullView(progress({ speedBps: 0 }), t).speed).toBeNull()
  })
})

describe('U5 · 按镜像拆开的明细', () => {
  const img = (over: Partial<PullProgress['images'][number]> = {}) => ({
    service: 'web',
    ref: 'ghcr.io/agentpit-io/hunter-community-web:1.2.3',
    shortRef: 'hunter-community-web',
    tag: '1.2.3',
    totalBytes: 748_000_000,
    downloadedBytes: 132_000_000,
    state: 'downloading' as const,
    sizeUnknown: false,
    seconds: null,
    ...over,
  })

  it('分母读不到时只说已下多少，**不给一个假的总量**', () => {
    // `sizeUnknown` 时 `totalBytes` 是观测值而不是真值，写成 `x / y` 会让人以为 y 是准的
    expect(pullImageBytes(img({ sizeUnknown: true }), t)).toBe('132 MB')
    expect(pullImageBytes(img(), t)).toBe('132 MB / 748 MB')
  })

  it('状态文字跟着 state 走', () => {
    expect(pullStateText('downloading', t)).toBe('下载中')
    expect(pullStateText('extracting', t)).toBe('解压中')
    expect(pullStateText('done', t)).toBe('完成')
    expect(pullStateText('failed', t)).toBe('失败')
    expect(pullStateText('pending', t)).toBe('等待')
  })
})
