import { createContext, useContext } from 'react'
import type { Event, State } from './machine'
import type { Dict, Locale } from '../i18n'
import type { BootState } from '../lib/types'

/** 叠在状态机之上的「抽屉页」：设置 / 日志 / 反馈 / 更新。关掉就回到状态机决定的页面。 */
export type Overlay = 'settings' | 'logs' | 'feedback' | 'update' | null

export interface Store {
  state: State
  send: (e: Event) => void
  locale: Locale
  setLocale: (l: Locale) => void
  t: Dict
  overlay: Overlay
  setOverlay: (o: Overlay) => void
  demo: boolean
  /**
   * 一句要带到下一页去的话（I11 · U1）。
   *
   * 目前唯一的来源：错误页复查发现「其实已经好了」，界面自己切到运行面板 ——
   * 切过去之后得有人告诉用户刚才发生了什么，否则那一下会像是界面自己乱跳。
   */
  notice: string | null
  setNotice: (n: string | null) => void
  /**
   * 开机那一次复查的结果（I12 · R1）。
   *
   * 「正在检查 Hunter 状态」那一页与运行面板都读它 —— 判定在 Rust 侧做完了，
   * 前端不重新判一遍（两处各判一次必然会出现两个不一样的结论）。
   * 演示模式下是 null。
   */
  boot: BootState | null
}

export const StoreCtx = createContext<Store | null>(null)

export function useStore(): Store {
  const v = useContext(StoreCtx)
  if (!v) throw new Error('useStore 必须在 StoreProvider 内部使用')
  return v
}

/** 只要文案的地方用这个，少写一次解构。 */
export function useT(): Dict {
  return useStore().t
}
