import { useEffect, useState } from 'react'
import { TitleBar } from './components/TitleBar'
import { Modal } from './components/Modal'
import { Button } from './components/Button'
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

export function App() {
  const { overlay, setOverlay } = useStore()
  const app = useAsync(() => ipc.appInfo(), [])

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
      <QuitDialog />
    </div>
  )
}

/**
 * 退出询问（技术方案 §5.8）：「容器还在跑，退出启动器时要不要一起停掉？」
 *
 * 两个来源都会走到这里：托盘的「退出」、窗口右上角的关闭按钮。
 * Rust 侧只在 `compose::is_up()` 为真时才发这个事件 —— 容器没在跑就直接退，不烦用户。
 */
function QuitDialog() {
  const { t } = useStore()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState<'keep' | 'stop' | null>(null)

  useEffect(() => {
    let un: (() => void) | undefined
    void ipc.onQuitRequest(() => setOpen(true)).then((f) => {
      un = f
    })
    return () => un?.()
  }, [])

  if (!open) return null

  async function quit(stop: boolean) {
    setBusy(stop ? 'stop' : 'keep')
    try {
      await ipc.quitApp(stop)
    } catch {
      // 退出命令发出去以后进程就没了，这里收不到有意义的错误
    }
  }

  return (
    <Modal
      testId="quit-dialog"
      title={t.quit.title}
      footer={
        <>
          <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => setOpen(false)}>
            {t.common.cancel}
          </Button>
          <Button
            size="sm"
            data-testid="quit-keep"
            disabled={busy !== null}
            onClick={() => void quit(false)}
          >
            {busy === 'keep' ? t.common.working : t.quit.keep}
          </Button>
          <Button
            size="sm"
            variant="primary"
            data-testid="quit-stop"
            disabled={busy !== null}
            onClick={() => void quit(true)}
          >
            {busy === 'stop' ? t.common.working : t.quit.stop}
          </Button>
        </>
      }
    >
      <p>{t.quit.body}</p>
      <ul className="mt-[12px] flex flex-col gap-[6px] text-sm text-muted">
        <li>· {t.quit.keepHint}</li>
        <li>· {t.quit.stopHint}</li>
      </ul>
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

function Overlay({ which }: { which: 'settings' | 'logs' | 'feedback' | 'update' }) {
  const { state } = useStore()
  if (which === 'settings') return <Settings />
  if (which === 'logs') return <Logs />
  if (which === 'update') return <Update />
  return <Feedback errorCode={state.name === 'Error' ? state.code : undefined} />
}
