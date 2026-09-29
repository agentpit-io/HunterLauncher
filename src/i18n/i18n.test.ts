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

/**
 * I17 · U2：**退出与停止必须是两件事，而且退出不再询问。**
 *
 * 用户 2026-09-29 的原话是「关闭应用默认不关闭服务，不要再提醒用户选择」。
 * 0.1.17 那条「要退出启动器吗？／保持后台运行／一起停止」的询问链路整个删掉了，
 * 这一组测试盯三件事：两件事的说法分得开、退出旁那句小字说清了「还在后台跑」、
 * 升级进行中那一句提示的两个动作都在。
 */
describe('I17 · U2 退出与停止分得开', () => {
  it('「停止服务」与「退出启动器」不是同一句话', () => {
    const d = zhCN.dashboard
    expect(d.stopService).not.toBe(d.quitLauncher)
    expect(d.startService).not.toBe(d.stopService)
    expect(d.quitLauncher.length).toBeGreaterThan(0)
    // 英文那边同样
    expect(en.dashboard.stopService).not.toBe(en.dashboard.quitLauncher)
  })

  it('退出旁边那行小字说清了「Hunter 继续在后台运行」', () => {
    for (const hint of [zhCN.dashboard.quitHint, zhCN.quit.hint]) {
      expect(hint).toContain('后台运行')
    }
    // 还得说明白想停服务该点哪儿 —— 用户不再被提醒容器占着内存，这是补偿
    expect(zhCN.quit.stopHint).toContain('停止服务')
  })

  it('升级进行中那一句有两个动作，且第二个是「回滚后再退」', () => {
    const q = zhCN.quitGuard
    expect(q.continueQuit).not.toBe(q.cancelAndQuit)
    expect(q.cancelAndQuit).toContain('回滚')
    expect(q.cancelAndQuit).toContain('退出')
    expect(en.quitGuard.continueQuit).not.toBe(en.quitGuard.cancelAndQuit)
  })
})

/**
 * I17 · P0-3：**「全停」那一种现场要有自己的话。**
 *
 * 客户 2026-09-29 那台（HL-GFV764）是「配置 1.2.3、镜像不齐、六个容器全停」——
 * 0.1.17 在这里返回 `None`，卡片压根不出现。现在它出得来，而且说法与
 * 「有容器在跑」那一档不同：全停时**不能**说「正在跑的 vX」，因为没有。
 */
describe('I17 · P0-3 中断卡片的两种现场', () => {
  it('有容器在跑 / 全停，两句话不一样', () => {
    const i = zhCN.interrupted
    const running = i.detail('1.2.3', '1.2.2')
    const stopped = i.detail('1.2.3', null)
    expect(running).not.toBe(stopped)
    // 全停那一句必须直说「都停着」，而不是编一个「正在跑的版本」
    expect(stopped).toContain('停')
    expect(stopped).not.toContain('正在跑')
    expect(running).toContain('1.2.2')
  })

  it('回退按钮两种说法不同，且全停时说的是「上一次成功的」', () => {
    const i = zhCN.interrupted
    expect(i.revertRunning('1.2.2')).not.toBe(i.revertStopped('1.2.2'))
    expect(i.revertStopped('1.2.2')).toContain('上一次成功')
    expect(i.revertHintStopped).not.toBe(i.revertHint)
  })
})

/**
 * I17 · U3 / U4：首页那个「检查更新」按钮，以及镜像源的新名字。
 */
describe('I17 · U3 / U4 文案', () => {
  it('查到新版本时按钮上带版本号，且与常态不是同一句', () => {
    const d = zhCN.dashboard
    expect(d.checkUpdateNew('v1.2.4')).toContain('v1.2.4')
    expect(d.checkUpdateNew('v1.2.4')).not.toBe(d.checkUpdate)
  })

  it('界面文案里不再出现「腾讯云香港」（I17 · U4 改名为「中国国内云服务」）', () => {
    const hits: string[] = []
    walk(dictOf('zh-CN'), '', (p, s) => {
      if (s.includes('腾讯云香港') || s.includes('腾讯云 · 香港')) hits.push(p)
    })
    expect(hits, `这些文案还写着旧名字：\n${hits.join('\n')}`).toEqual([])
  })
})
