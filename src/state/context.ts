import { createContext, useContext } from 'react'
import type { Event, State } from './machine'
import type { Dict, Locale } from '../i18n'

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
