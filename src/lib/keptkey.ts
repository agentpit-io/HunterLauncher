/**
 * 「上次保留下来的那把 key」该怎么用（I14 · F3）。
 *
 * 「只删除应用，保留数据」那一档把 `~/.hunter/app/.env` 留下了，界面上那句话是
 * 「以后重新安装会直接沿用，登录也不会失效」。0.1.13 没有兑现它：
 * 重装时 `--auto -y` 照样报 `E_KEY_INVALID: 标准输入不是终端…`。
 *
 * 判据抽成纯函数的理由和 `danger.ts` 的 `confirmOk` 一样：
 * **它决定界面上说哪一句话**，而说错话是这一类改动最容易出的问题 ——
 * 「沿用上次保留的 key」这句话要是在 key 其实用不了的时候说了，就是在骗人（红线 1）。
 */
import type { KeptKey } from './types'

/** 界面上该走哪一条路。 */
export type KeptKeyMode =
  /** 还在问 Rust（或者根本没有留下来的 key）：照常让用户填 */
  | 'ask'
  /** 有一把、而且刚验过是好的：直接沿用，不让用户重输 */
  | 'reuse'
  /** 有一把、但现在用不了：照常让用户填，**并且把原因摆出来** */
  | 'bad'
  /** `.env` 里留着东西、但它不是一把 hunter key 的样子：照常让用户填，也要说一句 */
  | 'shape'

/**
 * 注意「额度用完了」**不算**用不了：`reason === 'exhausted'` 时 `valid` 仍然是 true
 * （key 本身没问题，只是今天的额度用光了）。这把 key 照样能装、照样能登录，
 * 所以还是沿用它 —— 只是界面上要另外说一句额度的提醒。
 * 0.1.x 里有过一次把这两件事混起来的教训（见 I1 迭代报告用例 6）。
 */
export function keptKeyMode(k: KeptKey | null | undefined): KeptKeyMode {
  if (!k) return 'ask'
  // 形状不对的那一档 present 就是假的 —— 先认它，否则会被下一行当成「什么都没有」
  if (k.badShape) return 'shape'
  if (!k.present || !k.check) return 'ask'
  return k.check.valid ? 'reuse' : 'bad'
}

/** 沿用这把 key 的同时还要不要提一句「今天额度用完了」。 */
export function keptKeyExhausted(k: KeptKey | null | undefined): boolean {
  return keptKeyMode(k) === 'reuse' && k?.check?.reason === 'exhausted'
}
