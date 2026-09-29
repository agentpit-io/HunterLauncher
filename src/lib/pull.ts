/**
 * 升级 / 安装过程中那块字节进度**该显示什么**（I18 · U5）。
 *
 * ## 为什么单独一个文件
 *
 * 这里的每一行都是在守「不编数字」那条红线，而它最容易在改版时被顺手破坏。
 * 做成**纯函数**（不依赖 React、不碰 DOM）才能被 vitest 直接钉住 ——
 * 这个仓库的前端测试跑在 `environment: 'node'` 上，渲染组件测不了。
 *
 * ## 三条诚实性红线（方案 §3.3）
 *
 * 1. **总量没算出来时写「正在算…」，不写 0%。** `totalBytes` 要等 manifest 读完才有；
 *    在那之前任何百分比都是编的。
 * 2. **`speedBps` / `etaSeconds` 为 `null` 时写「剩余时间未知」**，
 *    **不许**拿「已用时 ÷ 已完成比例」去凑一个数 —— 那正是反复禁止的「编数字」。
 * 3. **「本次已下载」用的是 `netBytes`，不是 `downloadedBytes`。** 后者把
 *    「本机已有的层」也算进去了（它是进度条的分子）；本机全都有的机器上，
 *    拿 `downloadedBytes` 报数会说出「已下载 132 MB，用时 1 秒」而其实一个字节都没过网。
 */
import { bytes, duration } from './format'
import type { PullProgress } from './types'

/** `t.pull.*` 里这一块用得着的那几项。传进来而不是 import，纯函数才好测。 */
export interface PullTexts {
  sizeLine: (done: string, total: string) => string
  sizeComputing: string
  downloaded: (v: string) => string
  noDownload: string
  speedLine: (v: string) => string
  eta: (t: string) => string
  etaUnknown: string
  percentUnit: string
  retrying: (n: number) => string
  stateDownloading: string
  stateExtracting: string
  stateDone: string
  stateFailed: string
  statePending: string
}

/** 那一块上要显示的每一项。`null` = **这一项现在不该出现**（不是「显示 0」）。 */
export interface PullView {
  /** 总量那一行：`已下 / 总计`，或者「正在算…」，或者「本机都已有，没有下载」 */
  size: string
  /** 百分比。**分母没算出来、或者本机全都有时是 `null`** —— 不许显示 0% */
  percent: string | null
  /** 「本次已下载 X」（用 `netBytes`）。没下载到东西时是 `null` */
  net: string | null
  /** 当前网速。没有样本时是 `null` */
  speed: string | null
  /** 预计剩余。**`null` 这一项这里不会出现** —— 拿不到就显示「剩余时间未知」 */
  eta: string | null
  /** 第几次尝试。第一次时不显示 */
  attempt: string | null
  /** 拉完了没有（拉完了就不再显示「预计剩余」） */
  done: boolean
  /** 本机全都有、一个字节都没过网 */
  nothing: boolean
}

export function pullView(p: PullProgress, t: PullTexts): PullView {
  const done = p.phase === 'done'
  // 本机全都有：一个字节都没过网。**这时候不许出现「已下载 X」** ——
  // `downloadedBytes` 是 100%（本机已有的层都算进去了），拿它报数就是骗人
  const nothing = done && p.netBytes === 0

  const size = nothing
    ? t.noDownload
    : p.totalBytes > 0
      ? t.sizeLine(bytes(p.downloadedBytes), bytes(p.totalBytes))
      : t.sizeComputing

  return {
    size,
    // **分母没算出来就不给百分比**（不许 0%）；本机全都有时百分比也没有意义
    percent: !nothing && p.totalBytes > 0 ? `${p.percent}${t.percentUnit}` : null,
    net: !nothing && p.netBytes > 0 ? t.downloaded(bytes(p.netBytes)) : null,
    speed:
      !nothing && p.speedBps !== null && p.speedBps > 0
        ? t.speedLine(bytes(p.speedBps))
        : null,
    eta: done
      ? null
      : p.etaSeconds !== null && p.etaSeconds > 0
        ? t.eta(duration(p.etaSeconds))
        : t.etaUnknown,
    attempt: p.attempt > 1 ? t.retrying(p.attempt) : null,
    done,
    nothing,
  }
}

/** 一个镜像的状态文字。六个服务名与 `t.pull.roles` 是同一份名单。 */
export function pullStateText(
  state: PullProgress['images'][number]['state'],
  t: {
    stateDownloading: string
    stateExtracting: string
    stateDone: string
    stateFailed: string
    statePending: string
  },
): string {
  switch (state) {
    case 'downloading':
      return t.stateDownloading
    case 'extracting':
      return t.stateExtracting
    case 'done':
      return t.stateDone
    case 'failed':
      return t.stateFailed
    default:
      return t.statePending
  }
}

/**
 * 一个镜像那一行的字节部分。
 *
 * 分母读不到时（`sizeUnknown`）**只说已下多少，不给一个假的总量** ——
 * 那种情况下 `totalBytes` 是观测值而不是真值，写成 `x / y` 会让人以为 y 是准的。
 */
export function pullImageBytes(
  img: PullProgress['images'][number],
  t: Pick<PullTexts, 'sizeLine'>,
): string {
  return img.sizeUnknown
    ? bytes(img.downloadedBytes)
    : t.sizeLine(bytes(img.downloadedBytes), bytes(img.totalBytes))
}
