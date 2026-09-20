import { useEffect, useRef, useState } from 'react'
import { Button } from '../components/Button'
import { LogBox } from '../components/LogBox'
import { SegmentedControl, Toggle } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { copyText } from '../lib/clipboard'
import * as ipc from '../lib/ipc'
import { redact } from '../lib/mask'
import { useStore } from '../state/context'

const TAIL = 400
/** 实时滚动的轮询间隔。`docker compose logs --tail` 每次是一整个子进程，太密会很吵。 */
const LIVE_MS = 2000

/** compose 里的六个服务，顺序与视觉稿第 3 张的服务网格一致。 */
const SERVICES = ['web', 'api', 'opencode', 'llm-shim', 'postgres', 'redis'] as const
type Service = (typeof SERVICES)[number]
type Source = 'launcher' | 'compose'

/**
 * 日志页（里程碑 M3 第 4 项）：**按服务切换、实时滚动、复制、导出（脱敏）**。
 *
 * 三件事上刻意保守：
 *
 * 1. **脱敏在 Rust 侧就做过一遍了**（`compose::logs` 返回前统一过 `redact`），
 *    这里显示与复制时再过一遍 `mask.ts`。两道是有意的 —— 复制出去的内容一旦带 key
 *    就收不回来了（红线 2）。
 * 2. **导出走 Rust**，不是把界面上这份字符串存下来：Rust 那条路上有整包自查
 *    （`feedback::assert_clean`），扫出 key / 邮箱 / 完整 IP 就拒绝导出。
 * 3. **实时滚动只在用户滚到底部时才自动跟随**。正在往上翻历史的时候被拽回底部
 *    是最烦人的交互之一。
 */
export function Logs() {
  const { t, setOverlay } = useStore()
  const [source, setSource] = useState<Source>('launcher')
  const [service, setService] = useState<Service | 'all'>('all')
  const [live, setLive] = useState(true)
  const [lines, setLines] = useState<string[]>([])
  const [err, setErr] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // null = 还没点过。剪贴板用不了时要如实说，不能照样显示「已复制」（红线 1）
  const [copied, setCopied] = useState<boolean | null>(null)
  const boxRef = useRef<HTMLDivElement>(null)

  /** 拉一次日志。实时滚动开着的时候每 2 秒来一次。 */
  useEffect(() => {
    let alive = true
    const pull = () => {
      const p =
        source === 'launcher'
          ? ipc.launcherLog(TAIL)
          : ipc.composeLogs(service === 'all' ? undefined : service, TAIL)
      void p
        .then((v) => {
          if (!alive) return
          setLines(v.map(redact))
          setErr(null)
        })
        .catch((e: unknown) => {
          if (!alive) return
          setErr(e instanceof Error ? e.message : String(e))
        })
    }
    pull()
    if (!live) return () => {
      alive = false
    }
    const timer = window.setInterval(pull, LIVE_MS)
    return () => {
      alive = false
      window.clearInterval(timer)
    }
  }, [source, service, live])

  /** 已经在底部才自动跟随；用户往上翻历史时不打扰他。 */
  useEffect(() => {
    const el = boxRef.current
    if (!el || !live) return
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60
    if (nearBottom) el.scrollTop = el.scrollHeight
  }, [lines, live])

  async function doExport() {
    setBusy(true)
    setNote(null)
    try {
      const r = await ipc.exportLogs(source, service === 'all' ? undefined : service, TAIL)
      setNote(t.logs.exportedTo(r.path))
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <PlainLayout
      title={t.logs.title}
      right={
        <>
          <SegmentedControl<Source>
            value={source}
            testIdPrefix="logs-source"
            onChange={setSource}
            options={[
              { id: 'launcher', label: t.logs.sourceLauncher },
              { id: 'compose', label: t.logs.sourceCompose },
            ]}
          />
          <Button
            size="sm"
            data-testid="logs-copy"
            onClick={() => void copyText(lines.join('\n')).then(setCopied)}
          >
            {copied === null ? t.logs.copyAll : copied ? t.common.copied : t.common.copyFailed}
          </Button>
          <Button size="sm" data-testid="logs-export" disabled={busy} onClick={() => void doExport()}>
            {busy ? t.common.working : t.logs.exportFile}
          </Button>
          <Button size="sm" onClick={() => setOverlay(null)}>
            {t.common.close}
          </Button>
        </>
      }
      footer={
        <>
          <span className="text-sm text-muted">{note ?? t.logs.redactedNote}</span>
          <span className="tnum text-sm text-muted">
            {t.logs.lineCount(lines.length)} · {t.logs.tail(TAIL)}
          </span>
        </>
      }
    >
      <div className="flex h-full flex-col">
      {/* 服务切换只在「容器日志」这一档有意义：启动器自己的日志没有分服务一说 */}
      <div className="mb-[12px] flex shrink-0 flex-wrap items-center gap-[12px]">
        <span className="text-sm text-muted">{t.logs.serviceLabel}</span>
        <div className="flex flex-wrap gap-[6px]">
          <ServiceChip
            active={service === 'all'}
            disabled={source === 'launcher'}
            onClick={() => setService('all')}
            testId="logs-svc-all"
          >
            {t.logs.allServices}
          </ServiceChip>
          {SERVICES.map((sv) => (
            <ServiceChip
              key={sv}
              active={service === sv}
              disabled={source === 'launcher'}
              onClick={() => setService(sv)}
              testId={`logs-svc-${sv}`}
            >
              {sv}
            </ServiceChip>
          ))}
        </div>
        <div className="ml-auto flex items-center gap-[10px]">
          <span className="text-sm text-muted">{live ? t.logs.liveOn : t.logs.liveOff}</span>
          <Toggle on={live} onChange={setLive} label={t.logs.live} testId="logs-live" />
        </div>
      </div>

      <LogBox
        ref={boxRef}
        lines={lines}
        className="min-h-0 flex-1"
        emptyText={err ?? t.logs.empty}
      />
      </div>
    </PlainLayout>
  )
}

function ServiceChip({
  active,
  disabled,
  onClick,
  children,
  testId,
}: {
  active: boolean
  disabled?: boolean
  onClick: () => void
  children: React.ReactNode
  testId?: string
}) {
  return (
    <button
      type="button"
      data-testid={testId}
      disabled={disabled}
      onClick={onClick}
      className={`h-[28px] rounded-md border px-3 text-xs transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-35 ${
        active
          ? 'border-amber/60 bg-amber-soft text-amber-text'
          : 'border-line-strong bg-transparent text-body hover:bg-card hover:text-ink'
      }`}
    >
      {children}
    </button>
  )
}
