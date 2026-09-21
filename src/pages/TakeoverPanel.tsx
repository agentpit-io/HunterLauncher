import { useState } from 'react'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { EnvList } from '../components/EnvList'
import { LogBox } from '../components/LogBox'
import { Modal } from '../components/Modal'
import { StatusDot } from '../components/StatusDot'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import type { TakeoverOp } from '../lib/types'
import { useStore } from '../state/context'

/**
 * 接管态的运行面板（I7 · `reuse_existing_hunter`）。
 *
 * 用户在「你电脑上已经有一套 Hunter」那张卡片上点了「直接用它，不再装一套」之后，
 * 启动器就不再有自己的那一套 —— 这一页取代运行面板，管的是**他自己装的那一套**。
 *
 * ## 这一页与普通运行面板的三点不同
 *
 * 1. **数字更少。** 额度、数据源、今日对话这些要读我们自己 `.env` 才知道的东西，
 *    这里一概没有 —— 那一套不是启动器装的，我们没有它的 key，也不该去猜。
 *    能读到的只有 `docker ps` 给的东西：容器名、状态、镜像、端口。
 * 2. **每一个改动类按钮都要二次确认。** 文案由 Rust 给（`takeoverConfirmText`），
 *    界面不另写一套；而且就算界面被绕过去，Rust 侧 `confirmed=false` 也会拒。
 * 3. **没有「停止并删除」这类按钮。** 只有停止 / 启动 / 重启 —— 动作表里根本
 *    没有对别人那一套执行 `down` 的路（守卫的 `argv_takeover` 会拦）。
 */
export function TakeoverPanel() {
  const { t } = useStore()
  const st = useAsync(() => ipc.takeoverState(), [])
  const [busy, setBusy] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [confirm, setConfirm] = useState<{ op: TakeoverOp; text: string } | null>(null)
  const [logs, setLogs] = useState<string[] | null>(null)
  const [logErr, setLogErr] = useState<string | null>(null)

  const d = st.data

  async function ask(op: TakeoverOp) {
    setErr(null)
    setNote(null)
    try {
      setConfirm({ op, text: await ipc.takeoverConfirmText(op) })
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  async function run(op: TakeoverOp) {
    setBusy(op)
    setConfirm(null)
    try {
      setNote(await ipc.takeoverOp(op, true))
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
      st.reload()
    }
  }

  async function loadLogs() {
    setLogErr(null)
    try {
      setLogs(await ipc.takeoverLogs(undefined, 200))
    } catch (e) {
      setLogErr(e instanceof Error ? e.message : String(e))
      setLogs([])
    }
  }

  async function release() {
    setBusy('release')
    try {
      setNote(await ipc.takeoverRelease())
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
      st.reload()
    }
  }

  const up = (d?.containers ?? []).filter((c) => c.status.startsWith('Up')).length
  const total = d?.containers.length ?? 0

  return (
    <section
      data-testid="takeover-panel"
      className="flex min-h-0 flex-1 flex-col gap-gap overflow-y-auto bg-window px-panel pb-panel pt-[40px]"
    >
      <header className="flex shrink-0 items-start justify-between">
        <div className="flex items-center gap-[11px]">
          <StatusDot health={up > 0 ? 'healthy' : 'pending'} size={12} />
          <div>
            <h1 className="text-3xl font-semibold leading-none text-ink">{t.takeover.title}</h1>
            <div className="tnum mt-[14px] text-md leading-none text-body">
              {d?.project ? t.takeover.subline(d.project, up, total) : t.common.loading}
            </div>
          </div>
        </div>
        {d?.webUrl && (
          <Button
            variant="primary"
            data-testid="takeover-open"
            onClick={() => void ipc.openExternal(d.webUrl)}
          >
            {t.takeover.open}
          </Button>
        )}
      </header>

      {/* 这一句是整页的前提，**不能省**：管的是用户自己装的那一套 */}
      <div className="rounded-lg border border-amber/35 bg-amber-soft px-5 py-4 text-sm leading-[1.6] text-body">
        {t.takeover.banner}
      </div>

      {st.error && <div className="text-sm text-danger">{st.error.message}</div>}
      {d?.note && (
        <div
          data-testid="takeover-note"
          className="rounded-lg border border-warning/35 bg-card px-5 py-4 text-sm leading-[1.6] text-body"
        >
          {d.note}
        </div>
      )}

      <Card>
        <div className="text-md font-medium text-ink">{t.takeover.containers}</div>
        {total === 0 ? (
          <div className="mt-[12px] text-sm text-muted">{t.takeover.noContainers}</div>
        ) : (
          <div className="mt-[12px] flex flex-col gap-[6px]" data-testid="takeover-containers">
            {d?.containers.map((c) => (
              <div
                key={c.name}
                className="flex flex-wrap items-baseline gap-x-[14px] gap-y-[2px] rounded-md border border-line bg-card px-[14px] py-[9px]"
              >
                <span className="text-md text-ink">{c.name}</span>
                <span className="text-sm text-body">{c.status}</span>
                <span className="font-mono text-xs text-muted">{c.image}</span>
                <span className="font-mono text-xs text-dim">{c.ports || '—'}</span>
              </div>
            ))}
          </div>
        )}
        <div className="mt-[14px]">
          <EnvList
            items={[
              { label: t.takeover.project, value: d?.project || null, reason: t.common.loading },
              {
                label: t.takeover.workDir,
                value: d?.workingDir || null,
                // 还没读到 ≠ 读不到。**只有真的拿回来了、而且是空的**，
                // 才写那句「读不到（所以停止 / 重启做不了）」——
                // 加载中就说加载中，不要把「还没问完」说成「问不出来」
                reason: st.loading ? t.common.loading : t.takeover.noWorkDir,
              },
              { label: t.takeover.since, value: d?.since || null, reason: '—' },
            ]}
          />
        </div>
      </Card>

      <Card>
        <div className="text-md font-medium text-ink">{t.takeover.actions}</div>
        <div className="mt-[6px] text-sm leading-[1.6] text-muted">
          {d?.manageable ? t.takeover.actionsHint : t.takeover.readOnlyHint}
        </div>
        <div className="mt-[14px] flex flex-wrap gap-[10px]">
          <Button size="sm" data-testid="takeover-logs" onClick={() => void loadLogs()}>
            {t.takeover.viewLogs}
          </Button>
          {(['stop', 'start', 'restart'] as const).map((op) => (
            <Button
              key={op}
              size="sm"
              data-testid={`takeover-${op}`}
              disabled={!d?.manageable || busy !== null}
              onClick={() => void ask(op)}
            >
              {busy === op ? t.common.working : t.takeover[op]}
            </Button>
          ))}
          <Button
            size="sm"
            variant="ghost"
            data-testid="takeover-release"
            disabled={busy !== null}
            onClick={() => void release()}
          >
            {t.takeover.release}
          </Button>
        </div>
        {note && (
          <div className="mt-[12px] break-all text-sm leading-[1.5] text-amber-text" data-testid="takeover-msg">
            {note}
          </div>
        )}
        {err && <div className="mt-[12px] break-all text-sm text-danger">{err}</div>}
      </Card>

      {logs !== null && (
        <Card>
          <div className="text-md font-medium text-ink">{t.takeover.logsTitle}</div>
          {logErr && <div className="mt-[10px] text-sm text-danger">{logErr}</div>}
          <div className="mt-[12px]">
            <LogBox lines={logs} />
          </div>
        </Card>
      )}

      {/* 二次确认。**文案由 Rust 给**，界面不自己写一套 —— 两处各写一份迟早对不上 */}
      {confirm && (
        <Modal
          title={t.takeover[confirm.op]}
          testId="takeover-confirm"
          onClose={() => setConfirm(null)}
          footer={
            <>
              <Button onClick={() => setConfirm(null)}>{t.common.cancel}</Button>
              <Button
                variant="primary"
                data-testid="takeover-confirm-yes"
                onClick={() => void run(confirm.op)}
              >
                {t.takeover.confirmYes}
              </Button>
            </>
          }
        >
          <div className="text-md leading-[1.6] text-body" data-testid="takeover-confirm-text">
            {confirm.text}
          </div>
        </Modal>
      )}
    </section>
  )
}
