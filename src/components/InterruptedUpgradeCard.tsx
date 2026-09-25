import { useState } from 'react'
import { Button } from './Button'
import { AlertTriangle } from './Icons'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'

/**
 * **上一次升级没做完**（I16 · P1-2）。
 *
 * ## 这张卡片为什么存在
 *
 * 2026-09-25 客户升级卡在拉镜像，只能强退 —— 而强退发生在
 * 「`.env` 已经写成 1.2.2、镜像还没拉全」之后，**没有走到回滚**。
 * 机器于是停在一个说不清的中间态：
 *
 * ```text
 * 配置       → 1.2.2
 * 跑着的容器 → 1.2.0
 * 1.2.2 的镜像 → 不齐
 * ```
 *
 * 这时候如果照着配置去 `up -d`，docker 会去拉那个还没下完的镜像，
 * 用户**又看到一次「卡住」**。所以这一档必须先问人，给两条明确的出路。
 *
 * 判定在后端（`upgrade::judge_interrupted`），全部是实测量：
 * `.env` 里的 `HUNTER_VERSION`、`docker compose ps` 报的镜像 tag、
 * `docker image inspect` 查出来的本机镜像。这一页不猜任何一项。
 */
export function InterruptedUpgradeCard({ onResolved }: { onResolved?: () => void }) {
  const { t, setOverlay } = useStore()
  const [nonce, setNonce] = useState(0)
  const q = useAsync(() => ipc.interruptedUpgrade(), [nonce])
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [open, setOpen] = useState(false)
  const it = q.data

  if (!it) return null

  async function revert() {
    if (!it) return
    setBusy(true)
    setNote(null)
    try {
      setNote(await ipc.revertToRunning(it.runningTag))
      setNonce((n) => n + 1)
      onResolved?.()
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="mt-[14px] shrink-0 rounded-md border border-amber/45 bg-amber/5 px-4 py-3"
      data-testid="interrupted-upgrade"
    >
      <div className="flex items-start gap-2.5">
        <AlertTriangle size={16} className="mt-[3px] shrink-0 text-amber-text" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-md font-medium text-amber-text">{t.interrupted.title}</span>
            <span className="tnum shrink-0 rounded border border-line-strong px-1.5 py-[1px] text-xs text-muted">
              {t.interrupted.badge}
            </span>
          </div>
          <div className="mt-[6px] text-sm leading-[1.5] text-body">
            {t.interrupted.detail(it.configTag, it.runningTag)}
          </div>
          <div className="mt-[4px] text-xs leading-[1.5] text-muted">{t.interrupted.hint}</div>
          {note && (
            <div className="mt-[8px] text-sm leading-[1.5] text-amber-text" data-testid="interrupted-note">
              {note}
            </div>
          )}
          <div className="mt-[12px] flex flex-wrap items-center gap-[10px]">
            <Button
              size="sm"
              variant="primary"
              data-testid="interrupted-continue"
              disabled={busy}
              onClick={() => setOverlay('update')}
            >
              {t.interrupted.continueUpgrade(it.configTag)}
            </Button>
            <Button size="sm" data-testid="interrupted-revert" disabled={busy} onClick={() => void revert()}>
              {busy ? t.interrupted.reverting : t.interrupted.revert(it.runningTag)}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setOpen((v) => !v)}>
              {open ? t.interrupted.hideEvidence : t.interrupted.evidence}
            </Button>
            <span className="text-xs text-muted">{t.interrupted.revertHint}</span>
          </div>
          {open && (
            <pre className="selectable tnum mt-[10px] max-h-[180px] overflow-auto whitespace-pre-wrap break-all rounded-md border border-line bg-log px-3 py-2.5 text-xs leading-[1.45] text-dim">
              {it.lines.join('\n')}
            </pre>
          )}
        </div>
      </div>
    </div>
  )
}
