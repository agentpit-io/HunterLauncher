import { useState } from 'react'
import { Button } from './Button'
import { Modal } from './Modal'
import { usePoll } from '../lib/usePoll'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { useStore } from '../state/context'
import type { MonitorAlert } from '../lib/types'

/**
 * 运行面板顶上的异常提醒（I13 · R7）。
 *
 * 四条规矩，每一条都是方案 R7 写明的：
 *
 * 1. **最严重的那一条摆在外面**，其余折起来 —— 一次抛五条横幅等于一条都没说。
 * 2. **每条提醒都能展开看「凭什么这么说」**（`facts` 里全是实测数字）。
 *    这一条不是装饰：用户有权知道「系统盘只剩 3.2 GB」是从哪读出来的。
 * 3. **一键处理只做三件事**：清本项目旧镜像、悬空卷、过期备份。
 *    需要删用户文件 / 扩容 / 调内存的只出现在「建议」里，没有按钮。
 * 4. **规则层判不了的才给「让 AI 帮我看看」**（`needsAi`），
 *    点了才花额度，而且送出去的是脱敏指标快照。
 *
 * 「同一个问题 24 小时内不重复」由 Rust 侧的 `monitor-state.json` 负责 ——
 * 前端这里只负责显示当下真实存在的提醒。用户点「知道了」只影响这一次会话。
 */
export function AlertBanner() {
  const { t } = useStore()
  // 60 秒一次：这一层要跑 docker 与 sysinfo，比资源卡片贵得多，
  // 而磁盘满、反复重启这类事也不是秒级变化的
  const a = usePoll(() => ipc.monitorAlerts(), 60_000)
  const [hidden, setHidden] = useState<string[]>([])
  const [open, setOpen] = useState<MonitorAlert | null>(null)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [answer, setAnswer] = useState<string | null>(null)

  /** 打开详情时把上一条的 AI 回答清掉（放在点击处理里，不放 effect —— 见 eslint 的 set-state-in-effect）。 */
  function openDetails(x: MonitorAlert) {
    setOpen(x)
    setAnswer(null)
  }

  const alerts = (a.data?.alerts ?? []).filter((x) => !hidden.includes(x.id))
  if (alerts.length === 0) return null
  const top = alerts[0]!

  async function fix() {
    setBusy(true)
    try {
      const d = await ipc.cleanupRun()
      setNote(t.dashboard.alertCleanupDone(`${d.headline}（${bytes(d.freedBytes)}）`))
      a.reload()
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function ask(id: string) {
    setBusy(true)
    try {
      setAnswer(await ipc.assistResourceAsk(id))
    } catch (e) {
      setAnswer(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const crit = top.level === 'crit'
  return (
    <>
      <div
        className={`mt-[14px] shrink-0 rounded-md border px-4 py-2.5 ${
          crit ? 'border-danger/45 bg-danger-soft' : 'border-amber/45 bg-amber-soft'
        }`}
        data-testid="alert-banner"
      >
        <div className="flex items-start justify-between gap-[14px]">
          <div className="min-w-0">
            <div className={`text-sm font-medium ${crit ? 'text-danger' : 'text-amber-text'}`}>
              {top.title}
              {alerts.length > 1 && (
                <span className="ml-2 text-xs font-normal text-muted">
                  {t.dashboard.alertsMore(alerts.length - 1)}
                </span>
              )}
            </div>
            <div className="mt-[4px] text-sm leading-[1.45] text-body">{top.detail}</div>
          </div>
          <div className="flex shrink-0 flex-wrap gap-[10px]">
            <Button size="sm" variant="ghost" data-testid="alert-details" onClick={() => openDetails(top)}>
              {t.dashboard.alertDetails}
            </Button>
            {top.actions.includes('cleanup_project_space') || top.actions.includes('prune_backups') ? (
              <Button size="sm" data-testid="alert-fix" disabled={busy} onClick={() => void fix()}>
                {busy ? t.cleanup.working : t.dashboard.alertFix}
              </Button>
            ) : null}
            <Button size="sm" variant="ghost" onClick={() => setHidden((v) => [...v, top.id])}>
              {t.dashboard.alertDismiss}
            </Button>
          </div>
        </div>
        {note && <div className="mt-[8px] text-xs leading-[1.5] text-body">{note}</div>}
      </div>

      {open && (
        <Modal
          testId="alert-dialog"
          title={open.title}
          onClose={() => setOpen(null)}
          footer={
            <>
              {open.needsAi && (
                <Button
                  size="sm"
                  data-testid="alert-ask"
                  disabled={busy}
                  onClick={() => void ask(open.id)}
                >
                  {busy ? t.dashboard.alertAsking : t.dashboard.alertAsk}
                </Button>
              )}
              <Button size="sm" variant="primary" onClick={() => setOpen(null)}>
                {t.common.close}
              </Button>
            </>
          }
        >
          <p className="text-sm leading-[1.6] text-body">{open.detail}</p>
          <ul className="tnum mt-[12px] flex flex-col gap-[4px] text-xs leading-[1.5] text-muted">
            {open.facts.map((f) => (
              <li key={f}>· {f}</li>
            ))}
          </ul>
          {open.advice.length > 0 && (
            <>
              <div className="mt-[14px] text-sm text-label">{t.dashboard.alertAdvice}</div>
              <ul className="mt-[6px] flex flex-col gap-[6px] text-sm leading-[1.5] text-body">
                {open.advice.map((f) => (
                  <li key={f}>· {f}</li>
                ))}
              </ul>
            </>
          )}
          {answer && (
            <div
              className="mt-[14px] whitespace-pre-wrap rounded-md border border-line bg-window px-3 py-2 text-sm leading-[1.6] text-body"
              data-testid="alert-answer"
            >
              {answer}
            </div>
          )}
        </Modal>
      )}
    </>
  )
}
