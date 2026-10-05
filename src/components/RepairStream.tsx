import { useEffect, useMemo, useRef, useState } from 'react'
import * as demoData from '../lib/demo'
import * as ipc from '../lib/ipc'
import type { AssistEvent, AssistEventStatus } from '../lib/types'
import { useStore } from '../state/context'

/**
 * **升级时的「AI 正在排查」过程流**（I19 · P0-B）。
 *
 * ## 它为什么存在
 *
 * I19 之前，升级失败在界面上只有**一行红字**（`Update.tsx` 的 `upgrade-error`），
 * 没有「它在查什么、试了什么」的过程。而升级这一步失败之后，启动器其实是
 * 先自己排查处理的（规则优先、模型兜底，整轮最多 5 个修复回合）——
 * 那些事都推在同一条 `assist://event` 事件流上（与安装那条路同一个总线），
 * 这里把它们画出来。
 *
 * ## 两条纪律
 *
 * 1. **刷新不高于每秒一次**（沿用 I18 · U5 那条纪律）：`assist://event` 是逐条推的，
 *    监听器只把最新的塞进 ref，真正 `setState` 的是下面那个一秒定时器 ——
 *    不然升级这种吃 CPU 的时候整块面板会疯狂重渲。
 * 2. **页面自己不产生任何数字**：耗时、token、轮次全部来自事件流（红线 1）。
 */
export function RepairStream({ running = false }: { running?: boolean }) {
  const { t } = useStore()
  const buf = useRef<AssistEvent[]>([])
  const [events, setEvents] = useState<AssistEvent[]>([])

  // 换一轮升级要清掉上一次的过程流 —— 用方在挂载点上给 `key` 换一个值，
  // 整个组件重挂，状态自然归零（**不要**在 effect 里 setState：那会多渲染一轮）。
  useEffect(() => {
    let alive = true
    let un: (() => void) | undefined
    // 演示模式：`assist://event` 没有后端可推，四张卡直接铺进去（只在演示构建里）
    if (ipc.DEMO && ipc.demoPage() === 'update-repair-failed') {
      buf.current = demoRepairEvents()
    }
    const put = (e: AssistEvent) => {
      const i = buf.current.findIndex((x) => x.id === e.id)
      if (i < 0) buf.current = [...buf.current, e]
      else {
        const next = buf.current.slice()
        next[i] = e
        buf.current = next
      }
    }
    void ipc
      .onAssistEvent((e) => {
        if (alive) put(e)
      })
      .then((f) => {
        if (alive) un = f
        else f()
      })
    // 一秒一次把 ref 里最新的那一份搬上屏（不高于每秒一次）
    const id = window.setInterval(() => {
      setEvents((cur) =>
        cur.length === buf.current.length && cur.every((x, i) => x === buf.current[i])
          ? cur
          : buf.current.slice(),
      )
    }, 1000)
    return () => {
      alive = false
      un?.()
      window.clearInterval(id)
    }
  }, [])

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

  // 没在升级、这一轮也没发生任何自愈 → 整块不出现（不给正常升级加噪音）
  if (!running && roots.length === 0) return null

  return (
    <div
      data-testid="upgrade-repair-stream"
      className="mt-[10px] rounded-md border border-line bg-window px-3 py-2.5"
    >
      <div className="flex items-center gap-2">
        <span className="text-sm text-label">{t.update.repairTitle}</span>
        {roots.length === 0 && <span className="text-xs text-muted">{t.update.repairWaiting}</span>}
      </div>
      {roots.length > 0 && (
        <div className="mt-[8px] flex flex-col gap-[6px]">
          {roots.map((e) => (
            <Node key={e.id} e={e} childrenOf={childrenOf} t={t} />
          ))}
        </div>
      )}
    </div>
  )
}

/** 演示态那一份过程流。只有 VITE_DEMO=1 的开发构建走得到这里（发布包里进不来）。 */
function demoRepairEvents(): AssistEvent[] {
  return (demoData as { demoRepairEvents?: AssistEvent[] }).demoRepairEvents ?? []
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

/** 一条事件（含它下面挂的子事件）。与 `AutoInstall.tsx` 的树同构，只是更紧凑。 */
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
  return (
    <div style={{ paddingLeft: depth * 18 }}>
      <div
        data-testid={`repair-event-${e.id}`}
        className={`flex items-start gap-[9px] rounded-md border px-[11px] py-[7px] ${
          e.status === 'failed'
            ? 'border-danger/40 bg-danger-soft'
            : e.status === 'warn'
              ? 'border-warning/35 bg-card'
              : e.kind === 'review'
                ? 'border-info/55 bg-info-soft'
                : 'border-line bg-card'
        }`}
      >
        <span className={`mt-[5px] size-[7px] shrink-0 rounded-full ${DOT[e.status]}`} aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-[10px]">
            {/* 规则判的 vs 模型判的 —— 让用户看得见「这一句是怎么来的」 */}
            {e.by === 'rule' && (
              <span
                data-testid={`repair-by-${e.id}`}
                className="rounded-sm bg-success/20 px-[6px] py-[1px] text-xs leading-[1.6] text-success"
              >
                {t.update.repairRule}
              </span>
            )}
            {e.by === 'model' && (
              <span
                data-testid={`repair-by-${e.id}`}
                className="rounded-sm bg-info/25 px-[6px] py-[1px] text-xs leading-[1.6] text-info"
              >
                {t.update.repairModel}
              </span>
            )}
            {e.kind === 'review' && (
              <span className="rounded-sm bg-info/25 px-[6px] py-[1px] text-xs leading-[1.6] text-info">
                {t.update.repairReview}
              </span>
            )}
            <span className="text-sm leading-[1.35] text-ink">{e.title}</span>
            {e.detail && <span className="text-xs leading-[1.4] text-body">{e.detail}</span>}
          </div>
          {(hasTech || e.elapsedMs !== undefined || e.tokens !== undefined) && (
            <div className="mt-[3px] flex items-center gap-[10px]">
              {hasTech && (
                <button
                  type="button"
                  className="text-xs text-muted underline-offset-2 hover:text-body hover:underline"
                  onClick={() => setOpen((v) => !v)}
                >
                  {open ? t.update.repairHideTech : t.update.repairShowTech}
                </button>
              )}
              {e.elapsedMs !== undefined && (
                <span className="font-mono text-xs text-muted">{(e.elapsedMs / 1000).toFixed(1)}s</span>
              )}
              {e.tokens !== undefined && (
                <span className="font-mono text-xs text-muted">{e.tokens.toLocaleString('zh-CN')} token</span>
              )}
            </div>
          )}
          {open && hasTech && (
            <pre className="mt-[6px] whitespace-pre-wrap break-all rounded-md bg-log px-[10px] py-[7px] font-mono text-xs leading-[1.5] text-dim">
              {e.tech?.join('\n')}
            </pre>
          )}
        </div>
        <span className="mt-[1px] shrink-0 font-mono text-xs text-muted">{ICON[e.status]}</span>
      </div>
      {kids.length > 0 && (
        <div className="mt-[5px] flex flex-col gap-[5px]">
          {kids.map((k) => (
            <Node key={k.id} e={k} childrenOf={childrenOf} t={t} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  )
}
