import { useEffect, useMemo, useReducer, useState, type ReactNode } from 'react'
import { INITIAL_STATE, transition, type State } from './machine'
import { StoreCtx, type Overlay, type Store } from './context'
import { dictOf, guessLocale, type Locale } from '../i18n'
import * as ipc from '../lib/ipc'
import { DEMO, demoPage, demoSeed } from '../lib/ipc'

/** 演示模式下用 __HUNTER_DEMO_PAGE__ 直接打开某一页，给截图脚本用。 */
const DEMO_STATES: Record<string, State> = {
  welcome: { name: 'Welcome' },
  docker: { name: 'CheckDaemon' },
  'docker-missing': { name: 'InstallDockerGuide' },
  key: { name: 'NeedKey' },
  model: { name: 'ChooseModel' },
  consent: { name: 'Consent' },
  auto: { name: 'AutoInstalling' },
  'auto-need-user': { name: 'AutoInstalling' },
  start: { name: 'Starting' },
  done: { name: 'Done' },
  dashboard: { name: 'Ready' },
  'dashboard-quota-exhausted': { name: 'Ready' },
  // I7：升级上来的老机器（网页端口还对局域网开着）看到的那条横幅
  'dashboard-lan': { name: 'Ready' },
  settings: { name: 'Ready' },
  // I7：同一件事在设置页里的样子（只读说明 + 一键收紧）
  'settings-lan': { name: 'Ready' },
  logs: { name: 'Ready' },
  feedback: { name: 'Ready' },
  update: { name: 'Ready' },
  // I7：接管态的运行面板（App.tsx 里按 takeoverState().active 决定渲染哪一个）
  takeover: { name: 'Ready' },
  'auto-review': { name: 'AutoInstalling' },
  'auto-takeover-offer': { name: 'AutoInstalling' },
  error: {
    name: 'Error',
    code: 'E_START_TIMEOUT',
    from: 'Starting',
    detail: 'opencode 在 180 秒内没有变成 healthy（docker compose ps 的 Health 字段一直是 starting）',
  },
}

function initialState(): State {
  const page = demoPage()
  if (page && DEMO_STATES[page]) return DEMO_STATES[page]
  return transition(INITIAL_STATE, { type: 'BOOT' })
}

function initialOverlay(): Overlay {
  const page = demoPage()
  if (page === 'settings-lan') return 'settings'
  return page === 'settings' || page === 'logs' || page === 'feedback' || page === 'update'
    ? page
    : null
}

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, send] = useReducer(transition, undefined, initialState)
  // 演示模式固定中文，截图才对得上视觉稿；正常模式按系统语言猜一个初值
  const [locale, setLocale] = useState<Locale>(() => (DEMO ? 'zh-CN' : guessLocale()))
  const [overlay, setOverlay] = useState<Overlay>(initialOverlay)

  useEffect(() => {
    document.documentElement.lang = locale
  }, [locale])

  /**
   * 第二次打开启动器时直接进运行面板，不重走向导。
   * 判据来自 Rust 的 boot_state：`launcher.toml` 里 install.done 为真、`.env` 还在。
   * 演示模式不做这一步（截图脚本要能定到任意一页）。
   */
  useEffect(() => {
    if (DEMO || demoPage()) return
    let alive = true
    void ipc
      .bootState()
      .then((b) => {
        if (!alive || !b.installed) return
        setLocale(b.locale === 'en' ? 'en' : 'zh-CN')
        send({ type: 'RESUME_READY' })
      })
      .catch(() => {
        /* 读不到就老老实实走向导 */
      })
    return () => {
      alive = false
    }
  }, [])

  useEffect(() => {
    // 演示数据模式在 <html> 上留一个标记。它有两个用处：
    //   1) 排查时一眼能看出当前是不是演示模式；
    //   2) CI 靠 grep 这个字符串确认「发布产物里没有演示数据」—— 发布构建里 demoSeed 是
    //      编译期常量 null，整个分支连同字符串一起被摇掉，grep 必然搜不到（总控规则红线 1）。
    if (demoSeed) document.documentElement.dataset.hunterDemo = demoSeed.marker
  }, [])

  const value = useMemo<Store>(
    () => ({ state, send, locale, setLocale, t: dictOf(locale), overlay, setOverlay, demo: DEMO }),
    [state, locale, overlay],
  )

  return <StoreCtx.Provider value={value}>{children}</StoreCtx.Provider>
}
