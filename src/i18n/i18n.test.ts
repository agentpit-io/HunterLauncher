import { describe, expect, it } from 'vitest'
import zhCN from './zh-CN'
import en from './en'
import { LOCALES, dictOf } from './index'

/**
 * 界面上没有任何 Markdown 渲染器 —— 文案是**原样**打进 DOM 的。
 *
 * 所以文案里写 `**这样**` 不会变成粗体，用户看到的就是两个星号。
 * 这不是想象出来的风险：I7 收尾截图时，接管面板的横幅上真的印着
 * 「这一套是\*\*你自己装的\*\*」，设置页那条局域网说明也一样，
 * 在那之前它们已经连着两版看起来「读着没问题」—— 因为**没有人看过那张图**。
 *
 * 这条测试就是把那次发现钉死：靠代码，不靠「记得别写星号」。
 */
function walk(node: unknown, path: string, hit: (p: string, s: string) => void) {
  if (typeof node === 'string') return hit(path, node)
  if (Array.isArray(node)) return node.forEach((v, i) => walk(v, `${path}[${i}]`, hit))
  if (node && typeof node === 'object') {
    for (const [k, v] of Object.entries(node)) walk(v, path ? `${path}.${k}` : k, hit)
  }
  // 函数型文案（带参数的那些）在这里跳过：它们的字面量部分同样在这份字典里，
  // 但调用它们需要造参数，不值得为此把测试写复杂。
}

describe('文案里不许出现渲染不出来的标记', () => {
  for (const { id } of LOCALES) {
    it(`${id} 的每一条文案都没有字面量的 Markdown 粗体`, () => {
      const bad: string[] = []
      walk(dictOf(id), '', (p, s) => {
        if (s.includes('**')) bad.push(`${p}: ${s.slice(0, 60)}…`)
      })
      expect(bad, `这些文案会把星号原样印到界面上：\n${bad.join('\n')}`).toEqual([])
    })
  }

  it('两份字典的键完全一致（漏翻一条就会在界面上显示 undefined）', () => {
    const keys = (d: unknown) => {
      const out: string[] = []
      walk(d, '', (p) => out.push(p))
      return out.sort()
    }
    expect(keys(en)).toEqual(keys(zhCN))
  })
})

/**
 * R2 · 补跑（方案 §4.3）。
 *
 * 这台机器上系统定时任务根本没装上（`schtasks` 三条路全灭），补跑是
 * **启动器自己做的**。把它显示成「定时备份」就是在编 —— 两句话说的事不一样，
 * 用户得能从界面上分清「这一份是谁做的」。
 */
describe('R2 · 补跑的那一次不许混成定时备份', () => {
  it('四档备份的来源在中文里是四句不同的话', () => {
    const b = zhCN.backup
    const all = [b.kindManual, b.kindScheduled, b.kindMissed, b.kindPreUpgrade]
    expect(new Set(all).size, `四句话必须两两不同：${all.join(' / ')}`).toBe(4)
    expect(b.kindMissed).not.toContain('定时')
    expect(b.kindMissed.length).toBeGreaterThan(0)
  })

  it('英文那一侧同样分开', () => {
    const b = en.backup
    expect(b.kindMissed).not.toBe(b.kindScheduled)
    expect(new Set([b.kindManual, b.kindScheduled, b.kindMissed, b.kindPreUpgrade]).size).toBe(4)
  })
})

/**
 * R2 · U3：运行面板那行**常驻**状态（方案 §4.4 U3）。
 *
 * 两种机制的能力不一样 —— 系统定时任务「错过会补跑」，启动器补跑
 * 「错过了不补」。混成一句「已开启」就是在骗人，所以这里把它们钉成两句。
 */
describe('R2 · 自动备份那行状态分得清两条机制', () => {
  it('两句话不一样，而且补跑那句写明了等待多久', () => {
    const d = zhCN.dashboard
    const byFallback = d.autoByFallback(120)
    expect(byFallback).not.toBe(d.autoByScheduler)
    expect(byFallback).toContain('120')
    expect(d.autoByScheduler).not.toContain('120')
    // 「不补」那半句必须在 —— 这是两种机制真正的差别
    expect(byFallback).toContain('不补')
    expect(d.autoByScheduler).toContain('补跑')
    // 关着的时候是第三句，不能显示成「已开启」
    expect(d.autoOff).not.toBe(byFallback)
    expect(d.autoOff).not.toBe(d.autoByScheduler)
  })

  it('两条上限都写在常驻说明里', () => {
    const d = zhCN.dashboard
    expect(d.autoLimitNoRun).toContain('那天不会有备份')
    expect(d.autoLimitShortLived(120)).toContain('120')
    expect(d.autoLimitShortLived(120)).toContain('重新计时')
  })
})
