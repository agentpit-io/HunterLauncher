import { useState } from 'react'
import { AssistPanel } from '../components/AssistPanel'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { AlertTriangle } from '../components/Icons'
import { Modal } from '../components/Modal'
import { OfflineImport } from '../components/OfflineImport'
import { PlainLayout } from '../components/WizardLayout'
import * as ipc from '../lib/ipc'
import type { OneClickFeedback } from '../lib/types'
import { useStore } from '../state/context'
import type { ErrorCode } from '../state/machine'

/**
 * 通用错误页（技术方案第 18 节）。每个错误码一套标题 + 处理建议，
 * 下面永远有「一键反馈」（预填错误码）与「查看日志」两个出口。
 */
export function ErrorPage() {
  const { t, state, send, setOverlay } = useStore()
  if (state.name !== 'Error') return null

  const code = state.code as ErrorCode
  const info = t.error.codes[code] ?? t.error.codes.E_UNKNOWN

  return (
    <PlainLayout
      title={t.error.title}
      footer={
        <>
          <Button size="sm" onClick={() => setOverlay('logs')}>
            {t.error.viewLogs}
          </Button>
          <div className="flex gap-[10px]">
            <Button size="sm" onClick={() => send({ type: 'BACK' })}>
              {t.common.back}
            </Button>
            <Button size="sm" variant="primary" onClick={() => send({ type: 'RETRY' })}>
              {t.common.retry}
            </Button>
          </div>
        </>
      }
    >
      <Card className="max-w-[820px]">
        <div className="flex items-start gap-3">
          <AlertTriangle size={20} className="mt-[3px] shrink-0 text-danger" />
          <div className="min-w-0">
            <div className="text-2xl font-medium leading-tight text-ink">{info.title}</div>
            <p className="mt-[12px] text-md leading-[1.55] text-body">{info.hint}</p>
          </div>
        </div>

        <div className="mt-[22px] flex flex-col gap-[10px] border-t border-line pt-[18px]">
          <KV label={t.error.codeLabel} value={code} />
          {state.detail && <KV label={t.error.detailLabel} value={state.detail} />}
        </div>

        <div className="mt-[20px] flex flex-wrap gap-[10px]">
          {/* I7 · 一键反馈直达：生成脱敏包 + 预填 issue，**中间隔一屏给用户看**，
              他点了才打开浏览器。不会自动上传、不会假装上报成功。 */}
          <SendToDev code={code} detail={state.detail} />
          <Button size="sm" onClick={() => setOverlay('feedback')}>
            {t.error.oneClickFeedback}
          </Button>
          {/* 方案 §18 给 E_PULL_FAILED 规定的动作就是「换源重试 / 离线导入」。
              换源由「重试」那条路自己做（flow::pull 会自动换源 3 次），这里补上离线导入。 */}
          {code === 'E_PULL_FAILED' && <OfflineImport variant="button" />}
        </div>
      </Card>

      {/* 诊断助手（I4）：先跑确定性规则，认不出来才给「让 AI 帮我看看」。
          错误页是它最该出现的地方 —— 用户走到这里就是卡住了。 */}
      <AssistPanel
        className="mt-gap max-w-[820px]"
        errorCode={code}
        errorMessage={state.detail}
        stage={state.from}
      />
    </PlainLayout>
  )
}

/**
 * 「发送诊断给开发者」（I7）。
 *
 * 三步，**一步都不省**：
 * 1. 在本机生成脱敏诊断包（`export_zip` 那一套闸门 + 用户名/主机名扫描）；
 * 2. 把**将要贴出去的标题与正文原样摆给用户看**；
 * 3. 他点「打开 GitHub」才开浏览器。诊断包本身不自动上传，路径一并给出。
 *
 * 出口闸扫出东西时**不给「打开 GitHub」这个按钮** —— 宁可让这条路走不通，
 * 也不能把可能带着隐私的文字预填进一个公开页面。
 */
function SendToDev({ code, detail }: { code: string; detail?: string }) {
  const { t } = useStore()
  const [busy, setBusy] = useState(false)
  const [data, setData] = useState<OneClickFeedback | null>(null)
  const [err, setErr] = useState<string | null>(null)

  async function make() {
    setBusy(true)
    setErr(null)
    try {
      setData(await ipc.feedbackOneClick(code, detail))
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <Button
        size="sm"
        variant="secondary"
        data-testid="error-send-to-dev"
        disabled={busy}
        onClick={() => void make()}
      >
        {busy ? t.common.working : t.error.sendToDev}
      </Button>
      {err && <span className="self-center break-all text-sm text-danger">{err}</span>}
      {data && (
        <Modal
          title={t.error.sendToDevTitle}
          testId="send-to-dev"
          onClose={() => setData(null)}
          footer={
            <>
              <Button onClick={() => setData(null)}>{t.common.cancel}</Button>
              <Button
                size="sm"
                data-testid="send-to-dev-reveal"
                onClick={() => void ipc.revealPath(data.bundlePath)}
              >
                {t.error.revealBundle}
              </Button>
              {!data.scanHit && (
                <Button
                  variant="primary"
                  data-testid="send-to-dev-open"
                  onClick={() => {
                    void ipc.openExternal(data.issueUrl)
                    setData(null)
                  }}
                >
                  {t.error.openIssue}
                </Button>
              )}
            </>
          }
        >
          <p className="text-md leading-[1.6] text-body">{t.error.sendToDevIntro}</p>
          {data.scanHit && (
            <div
              data-testid="send-to-dev-scan-hit"
              className="mt-[12px] rounded-md border border-danger/45 bg-danger-soft px-[14px] py-[11px] text-sm leading-[1.6] text-danger"
            >
              {t.error.scanHit(data.scanHit)}
            </div>
          )}
          <div className="mt-[14px] text-sm text-label">{t.error.issueTitleLabel}</div>
          <div className="selectable mt-[4px] break-all text-md text-ink-2">{data.issueTitle}</div>
          <div className="mt-[14px] text-sm text-label">{t.error.issueBodyLabel}</div>
          <pre className="selectable mt-[4px] max-h-[220px] overflow-auto whitespace-pre-wrap break-all rounded-md bg-log px-[12px] py-[9px] font-mono text-xs leading-[1.5] text-dim">
            {data.issueBody}
          </pre>
          <div className="mt-[14px] text-sm leading-[1.6] text-muted">{data.note}</div>
        </Modal>
      )}
    </>
  )
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-4">
      <span className="w-[72px] shrink-0 text-md text-label">{label}</span>
      <span className="tnum selectable min-w-0 break-all text-md text-ink-2">{value}</span>
    </div>
  )
}
