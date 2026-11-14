import { useEffect, useRef, useState } from 'react'
import { Button } from '../components/Button'
import { Spinner } from '../components/Icons'
import { LogBox } from '../components/LogBox'
import { Modal } from '../components/Modal'
import { SegmentedControl, Toggle } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { copyText } from '../lib/clipboard'
import * as ipc from '../lib/ipc'
import { redact } from '../lib/mask'
import type { LogshipOutcome, LogshipPreview } from '../lib/types'
import { useStore } from '../state/context'

const TAIL = 400
/** 实时滚动的轮询间隔。`docker compose logs --tail` 每次是一整个子进程，太密会很吵。 */
const LIVE_MS = 2000

/** compose 里的六个服务，顺序与视觉稿第 3 张的服务网格一致。 */
const SERVICES = ['web', 'api', 'opencode', 'llm-shim', 'postgres', 'redis'] as const
type Service = (typeof SERVICES)[number]
type Source = 'launcher' | 'compose'

/**
 * 日志页（里程碑 M3 第 4 项 + I21 顶部一键上传与日志编码展示）：
 * **按服务切换、实时滚动、复制、导出（脱敏）+ 后台日志一键上传与日志编码横幅**。
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
export function Logs({
  initialOutcome = null,
  initialPreview = null,
}: {
  initialOutcome?: LogshipOutcome | null
  initialPreview?: LogshipPreview | null
}) {
  const { t, setOverlay, logUploadTrigger } = useStore()
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

  // I21：后台日志上传与日志编码（追踪码）状态。
  // 演示模式下 HUNTER_DEMO_PAGE=logs-uploaded 同步初始化上传完成态，确保首屏渲染直接可见日志编码。
  const isDemoUploaded = ipc.DEMO && ipc.demoPage() === 'logs-uploaded'
  const [uploading, setUploading] = useState(false)
  const [preview, setPreview] = useState<LogshipPreview | null>(initialPreview)
  const [outcome, setOutcome] = useState<LogshipOutcome | null>(() => {
    if (initialOutcome) return initialOutcome
    return isDemoUploaded
      ? {
          ok: true,
          traceCode: 'HL-DEMO01',
          truncated: false,
          message: '已上传。追踪码 HL-DEMO01，把它发给我们，我们就能查到这份日志。',
          status: 200,
          bundlePath: null,
          localLogPath: '~/.hunter/logs/launcher.log',
        }
      : null
  })
  const [uploadErr, setUploadErr] = useState<string | null>(null)
  const [showBodyModal, setShowBodyModal] = useState(false)
  const [traceCopied, setTraceCopied] = useState<boolean | null>(null)
  const [historyTrace, setHistoryTrace] = useState<{ code: string; at: string }>({ code: '', at: '' })
  const lastHandledTrigger = useRef<number>(0)

  // 读取历史最近一次上传的日志编码（供未触发本次上传但已有历史记录时展示）
  useEffect(() => {
    let alive = true
    void ipc
      .readSettings()
      .then((s) => {
        if (!alive) return
        if (s.lastTraceCode) {
          setHistoryTrace({ code: s.lastTraceCode, at: s.lastUploadAt })
        }
      })
      .catch(() => {
        // 读取设置失败不阻塞日志页
      })
    return () => {
      alive = false
    }
  }, [outcome])

  async function doUploadLogs() {
    setUploading(true)
    setUploadErr(null)
    setTraceCopied(null)
    try {
      const p = await ipc.logshipPreview({ stage: 'titlebar' })
      setPreview(p)
      if (p.scanHit) {
        setOutcome(null)
        setUploadErr(t.logship.scanHit(p.scanHit))
        return
      }
      const r = await ipc.logshipUpload()
      setOutcome(r)
    } catch (e) {
      setOutcome(null)
      setUploadErr(e instanceof Error ? e.message : String(e))
    } finally {
      setUploading(false)
    }
  }

  // 监听顶部栏「上传日志」按钮信号：一旦 logUploadTrigger > 0 且是新触发，立即自动上传后台日志
  useEffect(() => {
    if (logUploadTrigger > 0 && logUploadTrigger !== lastHandledTrigger.current) {
      lastHandledTrigger.current = logUploadTrigger
      void doUploadLogs()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [logUploadTrigger])

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
    if (!live)
      return () => {
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

  const activeTraceCode = outcome?.ok && outcome.traceCode ? outcome.traceCode : historyTrace.code || null
  const isFreshSuccess = Boolean(outcome?.ok && outcome.traceCode)
  const isFailed = Boolean(uploadErr || (outcome && !outcome.ok))

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
        {/* I21：顶部日志编码（追踪码）与一键上传状态横幅 */}
        <div
          data-testid="logs-trace-banner"
          className={`mb-[12px] flex shrink-0 flex-wrap items-center justify-between gap-[12px] rounded-md border px-[14px] py-[10px] ${
            isFailed
              ? 'border-danger/45 bg-danger-soft'
              : activeTraceCode
                ? 'border-amber/45 bg-amber-soft'
                : 'border-line bg-card'
          }`}
        >
          {uploading ? (
            <div className="flex items-center gap-[10px] text-sm text-body">
              <Spinner size={15} className="text-amber-text" />
              <span>{t.logship.autoUploading}</span>
            </div>
          ) : isFailed ? (
            <>
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium text-danger">{t.logship.failTitle}</div>
                <div className="mt-[2px] break-all text-xs leading-[1.5] text-body">
                  {uploadErr ?? outcome?.message}
                </div>
                {outcome?.localLogPath && (
                  <div className="tnum mt-[2px] text-xs text-muted">
                    {t.logship.localLog}：{outcome.localLogPath}
                  </div>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-[8px]">
                {outcome?.bundlePath && (
                  <Button size="sm" onClick={() => void ipc.revealPath(outcome.bundlePath ?? '')}>
                    {t.common.reveal}
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="primary"
                  data-testid="logs-reupload"
                  onClick={() => void doUploadLogs()}
                >
                  {t.logship.retryUpload}
                </Button>
              </div>
            </>
          ) : activeTraceCode ? (
            <>
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-[14px] gap-y-[4px]">
                <div className="flex items-baseline gap-[8px]">
                  <span className="text-xs text-label">{t.logship.bannerTraceLabel}</span>
                  <span
                    data-testid="logs-trace-code"
                    className="tnum selectable font-mono text-[20px] font-semibold leading-none tracking-[0.06em] text-amber-text"
                  >
                    {activeTraceCode}
                  </span>
                </div>
                <div className="min-w-[200px] flex-1 text-xs leading-[1.5] text-body">
                  {isFreshSuccess ? (
                    <>
                      <span>{t.logship.bannerSuccessHint}</span>
                      {historyTrace.at && (
                        <span className="tnum ml-[8px] text-muted">
                          {t.logship.lastTrace(activeTraceCode, historyTrace.at)}
                        </span>
                      )}
                    </>
                  ) : (
                    <span>
                      {historyTrace.at
                        ? `${t.logship.lastTrace(activeTraceCode, historyTrace.at)} · ${t.logship.bannerHistoryHint}`
                        : t.logship.bannerHistoryHint}
                    </span>
                  )}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-[8px]">
                <Button
                  size="sm"
                  variant="primary"
                  data-testid="logs-copy-trace"
                  onClick={() => void copyText(activeTraceCode).then(setTraceCopied)}
                >
                  {traceCopied === false
                    ? t.common.copyFailed
                    : traceCopied
                      ? t.common.copied
                      : t.logship.copyTraceShort}
                </Button>
                {preview && (
                  <Button
                    size="sm"
                    data-testid="logs-view-uploaded"
                    onClick={() => setShowBodyModal(true)}
                  >
                    {t.logship.viewUploadedBody}
                  </Button>
                )}
                <Button size="sm" data-testid="logs-reupload" onClick={() => void doUploadLogs()}>
                  {t.logship.reupload}
                </Button>
              </div>
            </>
          ) : (
            <>
              <div className="min-w-0 flex-1 text-xs leading-[1.5] text-muted">
                {t.logship.bannerIdleHint}
              </div>
              <Button
                size="sm"
                variant="secondary"
                data-testid="logs-reupload"
                onClick={() => void doUploadLogs()}
              >
                {t.logship.uploadNow}
              </Button>
            </>
          )}
        </div>

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

      {/* 查看刚才上传的脱敏正文弹窗 */}
      {showBodyModal && preview && (
        <Modal
          title={t.logship.viewUploadedBody}
          testId="logs-uploaded-body-modal"
          wide
          onClose={() => setShowBodyModal(false)}
          footer={
            <Button variant="primary" onClick={() => setShowBodyModal(false)}>
              {t.common.gotIt}
            </Button>
          }
        >
          <p className="text-sm leading-[1.6] text-body">{t.logship.intro}</p>
          <div className="mt-[10px] text-xs text-label">
            {t.logship.bodyLabel}（{t.logship.bodyBytes(preview.bodyBytes)}）
          </div>
          <pre
            data-testid="logs-uploaded-body"
            className="selectable tnum mt-[6px] max-h-[320px] overflow-auto whitespace-pre-wrap break-all rounded-md bg-log px-[12px] py-[9px] font-mono text-xs leading-[1.5] text-dim"
          >
            {preview.body}
          </pre>
        </Modal>
      )}
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
