import { useEffect, useState } from 'react'
import { TitleBar } from './components/TitleBar'
import { Modal } from './components/Modal'
import { Button } from './components/Button'
import { Booting } from './pages/Booting'
import { DataFound } from './pages/DataFound'
import { Welcome } from './pages/Welcome'
import { Docker } from './pages/Docker'
import { Key } from './pages/Key'
import { Model } from './pages/Model'
import { Consent } from './pages/Consent'
import { AutoInstall } from './pages/AutoInstall'
import { Start } from './pages/Start'
import { Done } from './pages/Done'
import { Dashboard } from './pages/Dashboard'
import { TakeoverPanel } from './pages/TakeoverPanel'
import { BackupPanel } from './pages/BackupPanel'
import { Settings } from './pages/Settings'
import { Logs } from './pages/Logs'
import { Feedback } from './pages/Feedback'
import { Update } from './pages/Update'
import { ErrorPage } from './pages/ErrorPage'
import { pageOf } from './state/machine'
import { useStore } from './state/context'
import { useAsync } from './lib/useAsync'
import * as ipc from './lib/ipc'
import type { Overlay as OverlayName } from './state/context'
import type { QuitGuard } from './lib/types'

export function App() {
  const { overlay, setOverlay, send } = useStore()
  const app = useAsync(() => ipc.appInfo(), [])

  /**
   * **修好了就接着装**（I9 的 P0-3）。
   *
   * 后端（规则层或 AI）跑完某个动作之后，只要 Docker 变成可用、而这一套还没装完，
   * 它就自己把安装主流程重新跑起来并发一条 `assist://resume`。界面这边要做的
   * 只有一件事：回到过程流那一页，别把用户留在「没能自动装好」那张卡片上。
   *
   * 0.1.8 在用户 Mac 上缺的正是这一步：14:54 内置运行时已经起来了、
   * docker 29.5.2 可用，界面却一直停在失败态，`~/.hunter/app/` 到最后是空的。
   */
  useEffect(() => {
    let un: (() => void) | undefined
    void ipc
      .onAssistResume(() => {
        setOverlay(null)
        send({ type: 'RESUME_INSTALL' })
      })
      .then((f) => {
        un = f
      })
    return () => un?.()
    // send / setOverlay 引用稳定
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 托盘「查看日志 / 反馈 / 设置」把界面带到对应的那一页
  useEffect(() => {
    let un: (() => void) | undefined
    void ipc
      .onNavigate((page) => {
        setOverlay(
          page === 'logs' || page === 'feedback' || page === 'settings' || page === 'update'
            ? (page as OverlayName)
            : null,
        )
      })
      .then((f) => {
        un = f
      })
    return () => un?.()
    // setOverlay 来自 useState，引用稳定；列进依赖只会让监听在每次渲染时重挂一遍
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div className="flex h-full flex-col overflow-hidden bg-window">
      <TitleBar info={app.data} />
      <main className="flex min-h-0 flex-1 flex-col">
        {overlay ? <Overlay which={overlay} /> : <Page />}
      </main>
      <UpgradeQuitGuard />
    </div>
  )
}

/**
 * **升级进行中那次退出被拦下了**（I17 · P0-4）。
 *
 * ## 为什么只有这一处会弹
 *
 * U2 把 0.1.17 那条「退出时问要不要一起停容器」的链路整个删掉了：退出就是退出，
 * 容器继续在后台跑，不问。**唯一的例外**是升级进行中 —— 客户 2026-09-29 那台
 * Windows 就是在这里点了「应用退出」：进程一死，那条会回滚的取消路径不会跑，
 * 机器于是停在「`.env` 已是 1.2.3、镜像不齐、容器全停」这种说不清的状态里。
 *
 * 常规退出**一个提示都不许有**（A4-6 专门防这个回归），所以这个组件只在后端
 * 明确发来事件时才出现；后端不发，它就一直什么都不做。
 *
 * ## 两个动作
 *
 * * **继续退出** —— 用户说了算，照旧退（`quitApp(true)` 绕过后端那道判定）。
 * * **取消升级并回滚后再退出** —— 复用既有的取消路径（`state.cancel`）：
 *   升级线程会杀拉取、把配置写回升级前那一份，做完后端自动退出。
 *   这一步要等几十秒，所以按钮置灰并写明在做什么。
 */
function UpgradeQuitGuard() {
  const { t } = useStore()
  const [g, setG] = useState<QuitGuard | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let un: (() => void) | undefined
    void ipc.onUpgradeQuitGuard(setG).then((f) => {
      un = f
    })
    return () => un?.()
  }, [])

  if (!g) return null

  async function forceQuit() {
    setBusy(true)
    try {
      await ipc.quitApp(true)
    } catch {
      // 退出命令发出去以后进程就没了，这里收不到有意义的错误
    }
  }

  async function cancelAndQuit() {
    setBusy(true)
    try {
      // 这个调用**不会返回**：后端等回滚收尾之后自己退进程。
      // 所以这里不能 await 完再收尾，置灰的状态就留在那儿 —— 正是想要的。
      await ipc.cancelUpgradeAndQuit()
    } catch {
      setBusy(false)
    }
  }

  return (
    <Modal
      testId="upgrade-quit-guard"
      title={t.quitGuard.title}
      onClose={() => setG(null)}
      footer={
        <>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => setG(null)}>
            {t.common.cancel}
          </Button>
          <Button size="sm" disabled={busy} data-testid="quit-guard-force" onClick={() => void forceQuit()}>
            {t.quitGuard.continueQuit}
          </Button>
          <Button
            size="sm"
            variant="primary"
            disabled={busy}
            data-testid="quit-guard-cancel-upgrade"
            onClick={() => void cancelAndQuit()}
          >
            {busy ? t.quitGuard.cancelling : t.quitGuard.cancelAndQuit}
          </Button>
        </>
      }
    >
      <p>{g.headline}</p>
      <p className="mt-[12px] leading-[1.6] text-muted">{g.body}</p>
      <p className="mt-[12px] text-xs leading-[1.5] text-muted">{t.quitGuard.cancelHint}</p>
    </Modal>
  )
}

function Page() {
  const { state } = useStore()
  // I7：接管态下，运行面板换成「管理你原有的那一套」那一页。
  // 判据来自 Rust 当场读的 `[takeover] project`，不是界面自己记的状态 ——
  // 用户可能是在上一次运行里做的选择。
  const takeover = useAsync(() => ipc.takeoverState(), [])
  switch (pageOf(state)) {
    // I12 · R1：开机第一屏。不超过 1 秒，查完由 StoreProvider 跳走
    case 'booting':
      return <Booting />
    case 'data-found':
      return <DataFound />
    case 'welcome':
      return <Welcome />
    case 'docker':
      return <Docker />
    case 'key':
      return <Key />
    case 'model':
      return <Model />
    case 'consent':
      return <Consent />
    case 'auto':
      return <AutoInstall />
    case 'start':
      return <Start />
    case 'done':
      return <Done />
    case 'dashboard':
      return takeover.data?.active ? <TakeoverPanel /> : <Dashboard />
    case 'error':
      return <ErrorPage />
  }
}

function Overlay({ which }: { which: 'settings' | 'logs' | 'feedback' | 'update' | 'backup' }) {
  const { state } = useStore()
  if (which === 'settings') return <Settings />
  if (which === 'logs') return <Logs />
  if (which === 'update') return <Update />
  if (which === 'backup') return <BackupPanel />
  return <Feedback errorCode={state.name === 'Error' ? state.code : undefined} />
}
