import { useState } from 'react'
import { Button, LinkButton } from './Button'
import { Modal } from './Modal'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { useStore } from '../state/context'
import type { OfflineImport as OfflineImportResult } from '../lib/types'

/**
 * 「从文件导入」（技术方案 §9 的离线包）。
 *
 * 两处用到它：
 *
 * * **选择模型页的底栏** —— 内网机器根本连不上任何镜像源，得在开始拉之前就有这个入口；
 * * **`E_PULL_FAILED` 错误页** —— 方案 §18 给这个错误码规定的动作就是「换源重试 / 离线导入」。
 *
 * 文件选择框由 Rust 侧弹（`pick_offline_tar`），前端不碰文件系统。
 * 导入完整时 Rust 会把镜像源与版本对齐到包里的那一套，下一步的拉取会整个跳过。
 */
export function OfflineImport({ variant = 'link' }: { variant?: 'link' | 'button' }) {
  const { t } = useStore()
  const [open, setOpen] = useState(false)

  return (
    <>
      {variant === 'link' ? (
        <LinkButton data-testid="offline-open" onClick={() => setOpen(true)}>
          {t.update.offlineTitle}
        </LinkButton>
      ) : (
        <Button size="sm" data-testid="offline-open" onClick={() => setOpen(true)}>
          {t.update.offlineTitle}
        </Button>
      )}
      {open && <OfflineDialog onClose={() => setOpen(false)} />}
    </>
  )
}

function OfflineDialog({ onClose }: { onClose: () => void }) {
  const { t } = useStore()
  const [busy, setBusy] = useState(false)
  const [r, setR] = useState<OfflineImportResult | null>(null)
  const [err, setErr] = useState<string | null>(null)

  async function pick() {
    setErr(null)
    let path: string | null = null
    try {
      path = await ipc.pickOfflineTar()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
      return
    }
    if (!path) return // 用户点了取消
    setBusy(true)
    try {
      setR(await ipc.importOffline(path))
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      testId="offline-dialog"
      title={t.update.offlineTitle}
      onClose={busy ? undefined : onClose}
      footer={
        <>
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>
            {t.common.close}
          </Button>
          <Button
            size="sm"
            variant="primary"
            data-testid="offline-pick"
            disabled={busy}
            onClick={() => void pick()}
          >
            {busy ? t.update.offlineImporting : t.update.offlinePick}
          </Button>
        </>
      }
    >
      <p className="leading-[1.6]">{t.update.offlineHint}</p>

      {err && <div className="mt-[14px] text-sm leading-[1.6] text-danger">{err}</div>}

      {r && (
        <div className="mt-[16px]">
          <div
            className={`text-sm leading-[1.6] ${r.complete ? 'text-amber-text' : 'text-danger'}`}
            data-testid="offline-result"
          >
            {r.complete
              ? t.update.offlineOk(r.matched.length, r.tag ?? '—')
              : t.update.offlineIncomplete(r.missing.join('、'))}
          </div>
          <ul className="tnum mt-[10px] flex flex-col gap-[5px] text-xs text-muted">
            {r.matched.map((m) => (
              <li key={m.service} className="flex justify-between gap-3">
                <span className="min-w-0 truncate">{m.reference}</span>
                <span className="shrink-0">{m.bytes ? bytes(m.bytes) : '—'}</span>
              </li>
            ))}
          </ul>
          {r.bytes > 0 && (
            <div className="tnum mt-[10px] break-all text-xs text-muted">
              {r.path} · {bytes(r.bytes)} · {r.seconds}s
            </div>
          )}
        </div>
      )}
    </Modal>
  )
}
