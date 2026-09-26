import { useEffect, useMemo, useReducer, useState, type ReactNode } from 'react'
import { INITIAL_STATE, transition, type State } from './machine'
import { StoreCtx, type Overlay, type Store } from './context'
import { dictOf, guessLocale, type Locale } from '../i18n'
import type { BootState } from '../lib/types'
import * as ipc from '../lib/ipc'
import { DEMO, demoPage, demoSeed } from '../lib/ipc'

/** 演示模式下用 __HUNTER_DEMO_PAGE__ 直接打开某一页，给截图脚本用。 */
const DEMO_STATES: Record<string, State> = {
  welcome: { name: 'Welcome' },
  // I12：开机那一页（不超过 1 秒，但截图要留档）与「检测到上次的数据」那一页
  booting: { name: 'Booting' },
  'data-found': { name: 'DataFound' },
  docker: { name: 'CheckDaemon' },
  'docker-missing': { name: 'InstallDockerGuide' },
  key: { name: 'NeedKey' },
  // I14 · F3：「只删除应用，保留数据」之后重装时的 key 页 ——
  // 上面是「沿用上次保留的 key」那张卡片，输入框收起来
  'key-kept': { name: 'NeedKey' },
  // 同上那一档旁边：留着的那一项不是一把 key 的样子，沿用不了，但界面上要说明为什么
  'key-shape': { name: 'NeedKey' },
  model: { name: 'ChooseModel' },
  consent: { name: 'Consent' },
  auto: { name: 'AutoInstalling' },
  'auto-need-user': { name: 'AutoInstalling' },
  start: { name: 'Starting' },
  done: { name: 'Done' },
  dashboard: { name: 'Ready' },
  'dashboard-quota-exhausted': { name: 'Ready' },
  // I13 · R7：运行面板顶上那条异常提醒（磁盘 / 反复重启）
  'dashboard-alert': { name: 'Ready' },
  // I13 · R6：备份与恢复那一页，以及它上面的「恢复数据」确认弹窗
  backup: { name: 'Ready' },
  'backup-restore': { name: 'Ready' },
  // I13 · R4：删除应用（三步里的前两步：选范围 + 逐字输入）
  uninstall: { name: 'Ready' },
  'uninstall-all': { name: 'Ready' },
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
  // I11：错误页复查发现「其实已经好了」。这一页会**自己跳到运行面板** ——
  // 截出来的图正是那一跳之后的样子（顶上那条绿色横幅），也就是 U1 要的证据
  'error-recovered': {
    name: 'Error',
    code: 'E_START_TIMEOUT',
    from: 'AutoInstalling',
    detail: 'opencode 在 180 秒内没有变成 healthy（docker compose ps 的 Health 字段一直是 starting）',
  },
  // I16：**拉着拉着不动了**（客户 Mac 上 0.1.15 那次的现场）。
  // 和 error（E_START_TIMEOUT）分开截一张：这一档的说法完全不同 ——
  // 不是「源不通」，是「连上了不给数据」，而且错误页上多一个「上传日志给我们」
  'error-stalled': {
    name: 'Error',
    code: 'E_PULL_STALLED',
    from: 'AutoInstalling',
    detail:
      '从「腾讯云 · 香港」拉了 1 分 30 秒，已下载 12.4 MB / 共 748 MB。这个源试过了 —— 连得上、就是不给数据。',
  },
  // I16：一键上传日志的两屏（预览 / 传完拿到追踪码），从运行面板的诊断区点开
  'upload-preview': { name: 'Ready' },
  'upload-done': { name: 'Ready' },
  // I16：强退之后留下的中间态 —— 运行面板顶上那张「上一次升级没做完」
  'dashboard-interrupted': { name: 'Ready' },
  // I9：上一次没装成功留下的内置运行时残骸（用户 Mac 上 0.1.8 那次的现场）。
  // 这一页同时也是「启动器已经替你做了这几步」那一块的样子
  'error-builtin': {
    name: 'Error',
    code: 'E_BUILTIN_DOWN',
    from: 'CheckDocker',
    detail:
      'Hunter 自己那套运行时装好了，虚拟机没起来。这不是你装的 Docker —— 它是上一次安装下到 ~/.hunter/runtime 里的，该走内置那条路把它起起来。',
  },
}

function initialState(): State {
  const page = demoPage()
  if (page && DEMO_STATES[page]) return DEMO_STATES[page]
  return transition(INITIAL_STATE, { type: 'BOOT' })
}

function initialOverlay(): Overlay {
  const page = demoPage()
  if (page === 'settings-lan' || page === 'settings-backup') return 'settings'
  // I13：「备份与恢复」是一张新的抽屉页
  if (page === 'backup' || page === 'backup-restore') return 'backup'
  return page === 'settings' || page === 'logs' || page === 'feedback' || page === 'update'
    ? page
    : null
}

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, send] = useReducer(transition, undefined, initialState)
  // 演示模式固定中文，截图才对得上视觉稿；正常模式按系统语言猜一个初值
  const [locale, setLocale] = useState<Locale>(() => (DEMO ? 'zh-CN' : guessLocale()))
  const [overlay, setOverlay] = useState<Overlay>(initialOverlay)
  // 「刚才其实已经好了」那句话，跨页带一下（I11 · U1）
  const [notice, setNotice] = useState<string | null>(null)
  // 开机那一次复查的结果（I12 · R1）。「正在检查」页与「检测到上次的数据」页都读它
  const [boot, setBoot] = useState<BootState | null>(null)

  useEffect(() => {
    document.documentElement.lang = locale
  }, [locale])

  /**
   * **打开启动器先查一次现状，查完再决定进哪一页**（I12 · R1）。
   *
   * 0.1.11 及之前这里只看 `install.done`，而且界面在等结果的那一小会儿
   * 已经把欢迎页画出来了 —— 用户 Mac 上 6/6 健康跑着，重开启动器看到的
   * 却是「欢迎使用 Hunter 启动器」。现在改成：
   *
   *   Idle →（BOOT）→ Booting「正在检查 Hunter 状态」→ 按后端给的 route 跳
   *
   * 三条路对应后端 `BootRoute` 的三个值，判据在 Rust 侧（现状优先，见 `boot_state`）。
   * 演示模式不做这一步（截图脚本要能定到任意一页）。
   */
  useEffect(() => {
    if (DEMO || demoPage()) return
    let alive = true
    void ipc
      .bootState()
      .then((b) => {
        if (!alive) return
        setLocale(b.locale === 'en' ? 'en' : 'zh-CN')
        setBoot(b)
        if (b.route === 'dashboard') {
          // 这一次替用户补写了安装标记 → 面板上要说一句刚才发生了什么
          if (b.adopted) setNotice(b.headline)
          send({ type: 'RESUME_READY' })
        } else if (b.route === 'data-found') {
          send({ type: 'BOOT_DATA_FOUND' })
        } else {
          send({ type: 'BOOT_FRESH' })
        }
      })
      .catch(() => {
        // 查不出来就老老实实走向导 —— 但**不能卡在「正在检查」那一页上**
        if (alive) send({ type: 'BOOT_FRESH' })
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
    () => ({
      state,
      send,
      locale,
      setLocale,
      t: dictOf(locale),
      overlay,
      setOverlay,
      demo: DEMO,
      notice,
      setNotice,
      boot,
    }),
    [state, locale, overlay, notice, boot],
  )

  return <StoreCtx.Provider value={value}>{children}</StoreCtx.Provider>
}
