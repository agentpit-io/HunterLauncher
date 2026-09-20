import { useEffect, useState } from 'react'
import { Button } from './Button'
import { Card } from './Card'
import { AlertTriangle, CheckCircle, Spinner } from './Icons'
import { copyText } from '../lib/clipboard'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'
import type { ActionPlan, AssistState } from '../lib/types'

/**
 * 诊断助手面板（I4 §三）。
 *
 * **两层的顺序由 Rust 定死，这一页只负责把结果画出来：**
 *
 *  1. 一挂载就调 `assist_diagnose` —— 第一层的确定性规则，零延迟、零 token。
 *  2. 规则层 `confident` 时**不显示**「让 AI 帮我看看」的主按钮：
 *     它已经有把握了，再去花用户的额度没有道理。仍然留一个不起眼的「还是不行」入口。
 *  3. AI 给的东西一律带琥珀色的「AI 建议」角标，和规则层的白底文案分得开 ——
 *     用户有权知道哪一句是规则说的、哪一句是模型说的。
 *  4. 会改动机器的动作**把完整命令原样显示出来**，用户点「确认执行」才跑。
 *  5. 被拒掉的动作也显示出来（模型提了什么、为什么没执行），不藏着。
 */
export function AssistPanel({
  errorCode,
  errorMessage,
  stage,
  className = '',
}: {
  errorCode?: string
  errorMessage?: string
  stage?: string
  className?: string
}) {
  const { t } = useStore()
  const [st, setSt] = useState<AssistState | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [copied, setCopied] = useState<boolean | null>(null)
  const [showReport, setShowReport] = useState(false)
  const [askedAnyway, setAskedAnyway] = useState(false)

  useEffect(() => {
    let alive = true
    void ipc
      .assistDiagnose({ errorCode, errorMessage, stage })
      .then((s) => alive && setSt(s))
      .catch((e) => alive && setErr(e instanceof Error ? e.message : String(e)))
    return () => {
      alive = false
      void ipc.assistReset()
    }
    // errorCode/stage 变了要重新诊断；errorMessage 只是附带信息，不进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [errorCode, stage])

  async function guard(tag: string, fn: () => Promise<AssistState>) {
    setBusy(tag)
    setErr(null)
    try {
      setSt(await fn())
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  if (err && !st) {
    return (
      <Card className={className}>
        <div className="flex items-start gap-2.5 text-sm leading-[1.5] text-danger">
          <AlertTriangle className="mt-[2px] shrink-0" />
          <span>{t.assist.collectFailed(err)}</span>
        </div>
      </Card>
    )
  }
  if (!st) {
    return (
      <Card className={className}>
        <div className="flex items-center gap-2.5 text-sm text-muted">
          <Spinner size={14} className="text-amber" />
          {t.assist.collecting}
        </div>
      </Card>
    )
  }

  const rule = st.rule
  const roundsLeft = st.maxRounds - st.rounds
  // 规则层有把握时不主动把人往 AI 那边引（零 token 能解决就别花钱）
  const showAskButton = st.enabled && (!rule.confident || askedAnyway) && roundsLeft > 0 && !st.done

  return (
    <Card className={className} data-testid="assist-panel">
      {/* ── 第一层 · 确定性规则 ───────────────────────────────────── */}
      <div className="flex items-start gap-2.5">
        {rule.confident ? (
          <CheckCircle size={16} className="mt-[3px] shrink-0 text-amber" />
        ) : (
          <AlertTriangle className="mt-[2px] shrink-0 text-muted" />
        )}
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-md font-medium text-ink">{rule.title}</span>
            <span className="tnum shrink-0 rounded border border-line-strong px-1.5 py-[1px] text-xs text-muted">
              {t.assist.ruleBadge}
            </span>
            {rule.code && <span className="tnum shrink-0 text-xs text-muted">{rule.code}</span>}
          </div>
          <div className="mt-2 whitespace-pre-wrap break-words text-sm leading-[1.55] text-body">
            {rule.detail}
          </div>
        </div>
      </div>

      {rule.actions.length > 0 && (
        <div className="mt-[14px] flex flex-col gap-[10px]">
          {rule.actions.map((a) => (
            <ActionRow
              key={a.id}
              action={a}
              busy={busy === `rule:${a.id}`}
              disabled={busy !== null}
              onRun={() => void guard(`rule:${a.id}`, () => ipc.assistRuleAction(a.id))}
            />
          ))}
        </div>
      )}

      {/* ── 第二层 · AI ────────────────────────────────────────────── */}
      {st.turns.map((turn) => (
        <div key={turn.round} className="mt-[16px] border-t border-line pt-[16px]">
          <div className="flex items-center gap-2">
            <span className="shrink-0 rounded border border-amber/45 bg-amber-soft px-1.5 py-[1px] text-xs text-amber-text">
              {t.assist.aiBadge}
            </span>
            <span className="tnum text-xs text-muted">
              {t.assist.roundLabel(turn.round, st.maxRounds)}
            </span>
            {turn.tokens !== null && (
              <span className="tnum text-xs text-muted">{t.assist.tokensThisRound(turn.tokens)}</span>
            )}
          </div>
          {turn.text && (
            <div className="mt-2 whitespace-pre-wrap break-words text-sm leading-[1.55] text-body">
              {turn.text}
            </div>
          )}

          {turn.ran.length > 0 && (
            <div className="mt-[12px] flex flex-col gap-2">
              <div className="text-xs text-muted">{t.assist.autoRan}</div>
              {turn.ran.map((r, i) => (
                <div key={i} className="rounded-md border border-line bg-log px-3 py-2">
                  <div className="flex items-center gap-2">
                    {r.ok ? (
                      <CheckCircle size={12} className="shrink-0 text-success" />
                    ) : (
                      <AlertTriangle size={12} className="shrink-0 text-danger" />
                    )}
                    <span className="text-xs text-body">{r.title}</span>
                  </div>
                  <div className="tnum mt-1 whitespace-pre-wrap break-all text-xs leading-[1.4] text-muted">
                    {r.command}
                  </div>
                </div>
              ))}
            </div>
          )}

          {/* 被拒掉的动作要显示出来：模型提了什么、为什么没执行，用户有权知道 */}
          {turn.rejected.length > 0 && (
            <div className="mt-[12px] flex flex-col gap-1.5" data-testid="assist-rejected">
              {turn.rejected.map((r, i) => (
                <div key={i} className="flex items-start gap-2 text-xs leading-[1.45] text-danger">
                  <AlertTriangle size={12} className="mt-[2px] shrink-0" />
                  <span>{t.assist.rejected(r.name, r.reason)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      ))}

      {/* 等用户确认的动作：完整命令原样展示 */}
      {st.pending.length > 0 && (
        <div className="mt-[14px] flex flex-col gap-[10px]">
          {st.pending.map((a) => (
            <ActionRow
              key={a.id}
              action={a}
              ai
              busy={busy === `ai:${a.id}`}
              disabled={busy !== null}
              onRun={() => void guard(`ai:${a.id}`, () => ipc.assistConfirm(a.id, true))}
            />
          ))}
        </div>
      )}

      {/* 降级：任何一条前提不满足都在这里说清楚原因 */}
      {st.degraded && (
        <div
          data-testid="assist-degraded"
          className="mt-[14px] flex items-start gap-2.5 rounded-md border border-line bg-log px-3 py-2.5 text-sm leading-[1.5] text-amber-text"
        >
          <AlertTriangle size={14} className="mt-[2px] shrink-0" />
          <span>{st.degraded.message}</span>
        </div>
      )}

      {/* ── 底部操作条 ─────────────────────────────────────────────── */}
      <div className="mt-[16px] flex flex-wrap items-center gap-[10px] border-t border-line pt-[14px]">
        {showAskButton && (
          <Button
            size="sm"
            variant="primary"
            data-testid="assist-ask"
            disabled={busy !== null}
            leading={busy === 'ask' ? <Spinner size={13} /> : undefined}
            onClick={() => void guard('ask', () => ipc.assistAsk())}
          >
            {st.turns.length === 0 ? t.assist.ask : t.assist.askAgain}
          </Button>
        )}
        {st.enabled && rule.confident && !askedAnyway && st.turns.length === 0 && (
          <Button size="sm" variant="ghost" data-testid="assist-still-broken" onClick={() => setAskedAnyway(true)}>
            {t.assist.stillBroken}
          </Button>
        )}
        <Button
          size="sm"
          variant="ghost"
          data-testid="assist-copy"
          onClick={() => {
            void copyText(st.reportText).then(setCopied)
          }}
        >
          {copied === true ? t.common.copied : copied === false ? t.common.copyFailed : t.assist.copyReport}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setShowReport((v) => !v)}>
          {showReport ? t.assist.hideReport : t.assist.showReport}
        </Button>
        <div className="ml-auto flex items-center gap-3 text-xs text-muted">
          {!st.enabled && <span>{t.assist.offHint}</span>}
          {st.enabled && !st.hasKey && <span>{t.assist.noKeyHint}</span>}
          {st.rounds > 0 && (
            <span className="tnum" data-testid="assist-tokens">
              {t.assist.tokensTotal(st.totalTokens, st.rounds, st.maxRounds)}
            </span>
          )}
        </div>
      </div>

      {err && <div className="mt-2 text-xs leading-[1.5] text-danger">{err}</div>}

      {showReport && (
        <pre className="selectable tnum mt-[12px] max-h-[260px] overflow-auto whitespace-pre-wrap break-all rounded-md border border-line bg-log px-3 py-2.5 text-xs leading-[1.45] text-dim">
          {st.reportText}
        </pre>
      )}
    </Card>
  )
}

/**
 * 一个动作。
 *
 * 会改动机器的（`mutating`）必须把**将要执行的完整命令**原样摆出来，
 * 配一句「要做什么 + 为什么」，用户点了「确认执行」才跑 ——
 * 这是 I4 明确要求的那一条，也是 Rust 侧 `actions::execute(_, confirmed)` 挡着的那一道。
 */
function ActionRow({
  action,
  ai = false,
  busy,
  disabled,
  onRun,
}: {
  action: ActionPlan
  ai?: boolean
  busy: boolean
  disabled: boolean
  onRun: () => void
}) {
  const { t } = useStore()
  const mutating = action.kind === 'mutating'
  const command = action.argv.length > 0 ? action.argv.map(quote).join(' ') : (action.summary ?? '')
  return (
    <div
      data-testid={`assist-action-${action.id}`}
      className={`rounded-md border px-3 py-2.5 ${mutating ? 'border-amber/40 bg-amber-soft/30' : 'border-line bg-log'}`}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium text-ink">{action.title}</span>
            {ai && (
              <span className="shrink-0 rounded border border-amber/45 px-1 py-[1px] text-xs text-amber-text">
                {t.assist.aiBadge}
              </span>
            )}
          </div>
          <div className="mt-1 text-xs leading-[1.45] text-muted">{action.why}</div>
          {command && (
            <div className="tnum mt-[7px] whitespace-pre-wrap break-all rounded border border-line bg-log px-2 py-1.5 text-xs leading-[1.4] text-dim">
              {command}
            </div>
          )}
        </div>
        <Button
          size="sm"
          variant={mutating ? 'primary' : 'secondary'}
          disabled={disabled}
          leading={busy ? <Spinner size={13} /> : undefined}
          onClick={onRun}
        >
          {mutating ? t.assist.confirmRun : t.assist.run}
        </Button>
      </div>
    </div>
  )
}

/** 只用于**显示**。真正执行走的是 Rust 侧的参数数组，永远不经过 shell（红线 3）。 */
function quote(a: string): string {
  return a.includes(' ') ? `"${a}"` : a
}
