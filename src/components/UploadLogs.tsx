import { useState } from 'react'
import { Button } from './Button'
import { Modal } from './Modal'
import { copyText } from '../lib/clipboard'
import * as ipc from '../lib/ipc'
import * as demoData from '../lib/demo'
import type { LogshipOutcome, LogshipPreview } from '../lib/types'
import { useStore } from '../state/context'

/**
 * **一键上传日志**（I16 · P0-2）。错误页、设置页、运行面板三个地方共用这一个按钮。
 *
 * ## 为什么要有它
 *
 * 2026-09-25 客户升级卡死，我们想查 —— 手上一份日志都没有，
 * 因为原来的路是「让他导诊断包 → 发文件给我们」，他一休息这条链子就断了。
 *
 * ## 三步，一步都不省
 *
 * 1. 点按钮 → 后端收一份**脱敏后**的正文，**这一步不发任何请求**；
 * 2. 那份正文**原样摆在屏幕上**给用户看（连 machineId 与附带的 meta 都摆出来）；
 * 3. 他点「确认上传」才发。传完把追踪码用一行大字显示出来，旁边一个复制按钮。
 *
 * 出口闸（key / 口令 / 邮箱 / IP / 用户名 / 主机名）扫出东西时**不给上传按钮** ——
 * 宁可这条路走不通，也不能把可能带隐私的东西发到我们自己的服务器上。
 */
export function UploadLogs({
  stage,
  errorCode,
  summary,
  size = 'sm',
  variant = 'secondary',
  testId = 'upload-logs',
}: {
  /** install / upgrade / pull / start / backup / uninstall … */
  stage?: string
  errorCode?: string
  summary?: string
  size?: 'sm' | 'md'
  variant?: 'primary' | 'secondary' | 'ghost'
  testId?: string
}) {
  const { t } = useStore()
  const [busy, setBusy] = useState<'preview' | 'upload' | null>(null)
  // 截图脚本用 HUNTER_DEMO_PAGE=upload-preview / upload-done 直接把这两屏打开 ——
  // 它们都在弹窗里，不先点一下按钮是截不到的
  const [preview, setPreview] = useState<LogshipPreview | null>(() =>
    ipc.DEMO && ipc.demoPage() === 'upload-preview' && testId === 'dashboard-upload-logs'
      ? demoPreview()
      : null,
  )
  const [outcome, setOutcome] = useState<LogshipOutcome | null>(() =>
    ipc.DEMO && ipc.demoPage() === 'upload-done' && testId === 'dashboard-upload-logs'
      ? demoOutcome()
      : null,
  )
  const [err, setErr] = useState<string | null>(null)
  const [copied, setCopied] = useState<boolean | null>(null)

  async function open() {
    setBusy('preview')
    setErr(null)
    setOutcome(null)
    try {
      setPreview(await ipc.logshipPreview({ stage, errorCode, summary }))
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  async function send() {
    setBusy('upload')
    setErr(null)
    try {
      const r = await ipc.logshipUpload()
      setOutcome(r)
      setPreview(null)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <>
      <Button size={size} variant={variant} data-testid={testId} disabled={busy !== null} onClick={() => void open()}>
        {busy === 'preview' ? t.common.working : t.logship.button}
      </Button>
      {err && !preview && !outcome && (
        <span className="self-center break-all text-sm text-danger" data-testid={`${testId}-error`}>
          {err}
        </span>
      )}

      {/* ① 预览：屏幕上这一份，就是会发出去的那一份 */}
      {preview && (
        <Modal
          title={t.logship.title}
          testId={`${testId}-preview`}
          wide
          onClose={() => setPreview(null)}
          footer={
            <>
              <Button onClick={() => setPreview(null)}>{t.common.cancel}</Button>
              {!preview.scanHit && (
                <Button
                  variant="primary"
                  data-testid={`${testId}-confirm`}
                  disabled={busy !== null}
                  onClick={() => void send()}
                >
                  {busy === 'upload' ? t.logship.uploading : t.logship.confirm}
                </Button>
              )}
            </>
          }
        >
          <p className="text-md leading-[1.6] text-body">{t.logship.intro}</p>
          {preview.scanHit && (
            <div
              data-testid={`${testId}-scan-hit`}
              className="mt-[12px] rounded-md border border-danger/45 bg-danger-soft px-[14px] py-[11px] text-sm leading-[1.6] text-danger"
            >
              {t.logship.scanHit(preview.scanHit)}
            </div>
          )}
          <div className="mt-[14px] flex flex-col gap-[6px]">
            <Row label={t.logship.endpoint} value={preview.endpoint} />
            <Row label={t.logship.machineId} value={preview.machineId} testId={`${testId}-machine-id`} />
            {preview.stage && <Row label={t.logship.stage} value={preview.stage} />}
            {preview.errorCode && <Row label={t.logship.errorCode} value={preview.errorCode} />}
            {preview.summary && <Row label={t.logship.summary} value={preview.summary} />}
            <Row label={t.logship.bodyLabel} value={t.logship.bodyBytes(preview.bodyBytes)} />
          </div>
          <div className="mt-[8px] text-xs leading-[1.5] text-muted">{t.logship.machineIdHint}</div>
          {preview.metaLines.length > 0 && (
            <>
              <div className="mt-[14px] text-sm text-label">{t.logship.metaLabel}</div>
              <ul className="tnum mt-[4px] flex flex-col gap-[3px] text-xs text-dim">
                {preview.metaLines.map((l) => (
                  <li key={l}>{l}</li>
                ))}
              </ul>
            </>
          )}
          {preview.truncated && (
            <div className="mt-[12px] text-xs leading-[1.5] text-amber-text">{t.logship.truncated}</div>
          )}
          <div className="mt-[14px] text-sm text-label">{t.logship.bodyLabel}</div>
          <pre
            data-testid={`${testId}-body`}
            className="selectable tnum mt-[4px] max-h-[260px] overflow-auto whitespace-pre-wrap break-all rounded-md bg-log px-[12px] py-[9px] font-mono text-xs leading-[1.5] text-dim"
          >
            {preview.body}
          </pre>
          {err && <div className="mt-[10px] break-all text-sm text-danger">{err}</div>}
        </Modal>
      )}

      {/* ② 结果：成了就把追踪码用一行大字摆出来；没成就说清楚东西还在哪 */}
      {outcome && (
        <Modal
          title={outcome.ok ? t.logship.doneTitle : t.logship.failTitle}
          testId={`${testId}-done`}
          onClose={() => setOutcome(null)}
          footer={
            <>
              {outcome.traceCode && (
                <Button
                  data-testid={`${testId}-copy`}
                  onClick={() => void copyText(outcome.traceCode ?? '').then(setCopied)}
                >
                  {copied === false ? t.common.copyFailed : copied ? t.common.copied : t.logship.copyTrace}
                </Button>
              )}
              {outcome.bundlePath && (
                <Button onClick={() => void ipc.revealPath(outcome.bundlePath ?? '')}>
                  {t.common.reveal}
                </Button>
              )}
              <Button variant="primary" onClick={() => setOutcome(null)}>
                {t.common.gotIt}
              </Button>
            </>
          }
        >
          {outcome.traceCode ? (
            <>
              <div className="text-sm text-label">{t.logship.traceLabel}</div>
              <div
                data-testid={`${testId}-trace`}
                className="tnum selectable mt-[6px] text-[34px] font-semibold leading-none tracking-[0.06em] text-amber-text"
              >
                {outcome.traceCode}
              </div>
              <p className="mt-[14px] text-md leading-[1.6] text-body">{t.logship.traceHint}</p>
            </>
          ) : (
            <p className="text-md leading-[1.6] text-body">{outcome.message}</p>
          )}
          {outcome.truncated && (
            <div className="mt-[12px] text-xs leading-[1.5] text-muted">{t.logship.truncated}</div>
          )}
          {!outcome.ok && (
            <div className="mt-[14px] flex flex-col gap-[6px]">
              <Row label={t.logship.localLog} value={outcome.localLogPath} />
              {outcome.status !== null && <Row label="HTTP" value={String(outcome.status)} />}
            </div>
          )}
        </Modal>
      )}
    </>
  )
}

/** 演示态那两屏。只有 VITE_DEMO=1 的开发构建走得到这里（发布包里进不来）。 */
function demoPreview(): LogshipPreview | null {
  return (demoData as { demoLogshipPreview?: LogshipPreview }).demoLogshipPreview ?? null
}
function demoOutcome(): LogshipOutcome | null {
  return (demoData as { demoLogshipOutcome?: LogshipOutcome }).demoLogshipOutcome ?? null
}

function Row({ label, value, testId }: { label: string; value: string; testId?: string }) {
  return (
    <div className="flex gap-4">
      <span className="w-[112px] shrink-0 text-sm text-label">{label}</span>
      <span data-testid={testId} className="tnum selectable min-w-0 break-all text-sm text-ink-2">
        {value}
      </span>
    </div>
  )
}
