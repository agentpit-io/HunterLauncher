import { useEffect, useMemo, useRef, useState } from 'react'
import { Button } from '../components/Button'
import { WizardLayout } from '../components/WizardLayout'
import { Spinner } from '../components/Icons'
import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'
import type { AssistEvent, AssistEventStatus, AssistSummary } from '../lib/types'
import { ERROR_CODES, type ErrorCode } from '../state/machine'

/**
 * 实时过程流（I5 · AI 自动驾驶安装方案 §七）。
 *
 * **这一页替代了原来的「拉取镜像」与「启动」两页**：授权之后到六个服务健康为止，
 * 用户一次都不用点 —— 除非撞上「需要你」的三种情况之一（那时会出现一张金色描边的卡片，
 * 上面只有两个按钮，文案是结论不是问句）。
 *
 * 页面自己不产生任何数字：耗时、端口、token、进度全部来自 Rust 推过来的事件
 * （红线 1）。事件里没有的，这里就不显示，不拿占位数字凑。
 */
export function AutoInstall() {
  const { t, send } = useStore()
  const [events, setEvents] = useState<AssistEvent[]>([])
  const [summary, setSummary] = useState<AssistSummary | null>(null)
  const [startError, setStartError] = useState<string | null>(null)
  const started = useRef(false)
  const scroller = useRef<HTMLDivElement | null>(null)

  // 订阅 + 拉快照 + 开跑。顺序很要紧：**先订阅再开跑**，
  // 否则头几条事件（「检查 Docker」）会在监听挂上之前就发出去了。
  useEffect(() => {
    let alive = true
    const offs: (() => void)[] = []

    function put(e: AssistEvent) {
      setEvents((prev) => {
        const i = prev.findIndex((x) => x.id === e.id)
        if (i < 0) return [...prev, e]
        const next = prev.slice()
        next[i] = e
        return next
      })
    }

    async function boot() {
      offs.push(await ipc.onAssistEvent((e) => alive && put(e)))
      offs.push(await ipc.onAssistSummary((s) => alive && setSummary(s)))
      offs.push(
        await ipc.onAssistDone((o) => {
          if (!alive) return
          if (o.ok) send({ type: 'AUTO_DONE' })
          else
            send({
              type: 'AUTO_FAILED',
              code: toErrorCode(o.code),
              detail: o.message ?? undefined,
            })
        }),
      )
      // 页面可能是重进的（从错误页点「重试」），先把已经发生过的补齐
      const snap = await ipc.assistAutoSnapshot()
      if (!alive) return
      snap.events.forEach(put)
      if (snap.summary.phase) setSummary(snap.summary)
      if (!snap.running && !started.current) {
        started.current = true
        try {
          await ipc.assistAutoStart()
        } catch (e) {
          if (alive) setStartError(e instanceof Error ? e.message : String(e))
        }
      }
    }
    void boot()
    return () => {
      alive = false
      offs.forEach((f) => f())
    }
    // send 来自 store，引用稳定
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 新事件来了滚到底 —— 用户要看的是「现在在干什么」
  useEffect(() => {
    const el = scroller.current
    if (el) el.scrollTop = el.scrollHeight
  }, [events.length])

  const roots = useMemo(() => events.filter((e) => e.parent === null), [events])
  const childrenOf = useMemo(() => {
    const m = new Map<number, AssistEvent[]>()
    for (const e of events) {
      if (e.parent === null) continue
      const list = m.get(e.parent) ?? []
      list.push(e)
      m.set(e.parent, list)
    }
    return m
  }, [events])

  const waiting = events.find((e) => e.status === 'waiting' && (e.choices?.length ?? 0) > 0)

  return (
    <WizardLayout title={t.auto.title} intro={t.auto.intro}>
      <TopBar summary={summary} t={t} />

      {startError && (
        <div className="mt-[14px] rounded-lg border border-danger/45 bg-danger-soft px-5 py-3 text-sm text-danger">
          {startError}
        </div>
      )}

      {/* 「需要你」那张卡片出现时把树的高度压一压 —— 卡片必须完整露出来，
          不然用户要先滚动才看得到那两个按钮，「不用点」就成了「先滚再点」 */}
      <div
        ref={scroller}
        className={`mt-[16px] flex flex-col gap-[8px] overflow-y-auto pr-1 ${
          waiting ? 'max-h-[240px]' : 'max-h-[380px]'
        }`}
      >
        {roots.length === 0 && !startError && (
          <div className="flex items-center gap-2 text-md text-muted">
            <Spinner size={14} />
            {t.auto.starting}
          </div>
        )}
        {roots.map((e) => (
          <Node key={e.id} e={e} childrenOf={childrenOf} t={t} />
        ))}
      </div>

      {waiting && <NeedUser e={waiting} t={t} />}

      <div className="mt-[18px] flex items-center gap-4">
        <Button
          size="sm"
          variant="ghost"
          data-testid="auto-cancel"
          onClick={() => void ipc.cancelInstall()}
        >
          {t.auto.stop}
        </Button>
        <span className="text-xs text-muted">{t.auto.stopHint}</span>
      </div>
    </WizardLayout>
  )
}

/** 顶上那一行常驻摘要（方案 §二末尾）。数字全部来自事件流。 */
function TopBar({ summary, t }: { summary: AssistSummary | null; t: ReturnType<typeof useStore>['t'] }) {
  return (
    <div
      data-testid="auto-summary"
      className="mt-[24px] flex flex-wrap items-center gap-x-[18px] gap-y-[6px] rounded-lg border border-amber/35 bg-amber-soft px-5 py-[13px]"
    >
      <span className="flex items-center gap-2 text-md text-amber-text">
        <Spinner size={13} />
        {summary?.phase ? t.auto.working(summary.phase) : t.auto.workingPlain}
      </span>
      <span className="text-sm text-body">{t.auto.solved(summary?.solved ?? 0)}</span>
      <span className="font-mono text-sm text-dim">{t.auto.tokens(summary?.tokens ?? 0)}</span>
      <span className="font-mono text-sm text-dim">
        {t.auto.rounds(summary?.rounds ?? 0, summary?.maxRounds ?? 0)}
      </span>
      {summary && summary.elapsedMs > 0 && (
        <span className="font-mono text-sm text-dim">{t.auto.elapsed(Math.round(summary.elapsedMs / 1000))}</span>
      )}
    </div>
  )
}

const DOT: Record<AssistEventStatus, string> = {
  running: 'bg-amber animate-pulse',
  ok: 'bg-success',
  warn: 'bg-warning',
  failed: 'bg-danger',
  waiting: 'bg-amber',
  skipped: 'bg-line-strong',
}

const ICON: Record<AssistEventStatus, string> = {
  running: '…',
  ok: '✓',
  warn: '!',
  failed: '✕',
  waiting: '?',
  skipped: '–',
}

function Node({
  e,
  childrenOf,
  t,
  depth = 0,
}: {
  e: AssistEvent
  childrenOf: Map<number, AssistEvent[]>
  t: ReturnType<typeof useStore>['t']
  depth?: number
}) {
  const [open, setOpen] = useState(false)
  const kids = childrenOf.get(e.id) ?? []
  const hasTech = (e.tech?.length ?? 0) > 0
  // 「需要你」的卡片单独渲染在下面，树里不重复画按钮
  return (
    <div style={{ paddingLeft: depth * 22 }}>
      <div
        data-testid={`auto-event-${e.id}`}
        className={`flex items-start gap-[10px] rounded-md border px-[14px] py-[9px] ${
          e.status === 'failed'
            ? 'border-danger/40 bg-danger-soft'
            : e.status === 'waiting'
              ? 'border-amber/60 bg-amber-soft'
              : e.status === 'warn'
                ? 'border-warning/35 bg-card'
                : 'border-line bg-card'
        }`}
      >
        <span className={`mt-[6px] size-[8px] shrink-0 rounded-full ${DOT[e.status]}`} aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-[12px]">
            <span className="text-md leading-[1.35] text-ink">{e.title}</span>
            {e.detail && <span className="text-sm leading-[1.4] text-body">{e.detail}</span>}
          </div>
          {(hasTech || e.elapsedMs !== undefined || e.tokens !== undefined) && (
            <div className="mt-[4px] flex items-center gap-[12px]">
              {hasTech && (
                <button
                  type="button"
                  className="text-xs text-muted underline-offset-2 hover:text-body hover:underline"
                  onClick={() => setOpen((v) => !v)}
                >
                  {open ? t.auto.hideTech : t.auto.showTech}
                </button>
              )}
              {e.elapsedMs !== undefined && (
                <span className="font-mono text-xs text-muted">{(e.elapsedMs / 1000).toFixed(1)}s</span>
              )}
              {e.tokens !== undefined && (
                <span className="font-mono text-xs text-muted">{t.auto.tokens(e.tokens)}</span>
              )}
            </div>
          )}
          {open && hasTech && (
            <pre className="mt-[8px] whitespace-pre-wrap break-all rounded-md bg-log px-[12px] py-[9px] font-mono text-xs leading-[1.5] text-dim">
              {e.tech?.join('\n')}
            </pre>
          )}
        </div>
        <span className="mt-[2px] shrink-0 font-mono text-xs text-muted">{ICON[e.status]}</span>
      </div>
      {kids.length > 0 && (
        <div className="mt-[6px] flex flex-col gap-[6px]">
          {kids.map((k) => (
            <Node key={k.id} e={k} childrenOf={childrenOf} t={t} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  )
}

/** 「需要你」——**只有三种情况会出现**（方案 §三）。按钮文案是结论不是问句。 */
function NeedUser({ e, t }: { e: AssistEvent; t: ReturnType<typeof useStore>['t'] }) {
  const [busy, setBusy] = useState(false)
  return (
    <div
      data-testid="auto-need-user"
      className="mt-[16px] rounded-lg border-2 border-amber bg-amber-soft px-5 py-4"
    >
      <div className="text-md font-medium leading-tight text-amber-text">{t.auto.needUser}</div>
      <div className="mt-[6px] text-md leading-[1.5] text-ink">{e.title}</div>
      {e.detail && <div className="mt-[4px] text-sm leading-[1.5] text-body">{e.detail}</div>}
      <div className="mt-[14px] flex items-center gap-3">
        {(e.choices ?? []).map((c) => (
          <Button
            key={c.value}
            size="sm"
            variant={c.primary ? 'primary' : 'secondary'}
            data-testid={`auto-choice-${c.value}`}
            disabled={busy}
            onClick={() => {
              setBusy(true)
              void ipc.assistAutoAnswer(c.value).finally(() => setBusy(false))
            }}
          >
            {c.label}
          </Button>
        ))}
      </div>
    </div>
  )
}

/** Rust 给的错误码字符串 → 状态机认得的那一个。认不得就归到 E_UNKNOWN，不猜。 */
function toErrorCode(code: string | null): ErrorCode {
  const c = (code ?? '') as ErrorCode
  return ERROR_CODES.includes(c) ? c : 'E_UNKNOWN'
}
