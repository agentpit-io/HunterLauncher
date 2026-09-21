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
