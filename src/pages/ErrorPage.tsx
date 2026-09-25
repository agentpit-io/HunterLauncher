import { useEffect, useState } from 'react'
import { AssistPanel } from '../components/AssistPanel'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { AlertTriangle, CheckCircle, Spinner } from '../components/Icons'
import { Modal } from '../components/Modal'
import { OfflineImport } from '../components/OfflineImport'
import { UploadLogs } from '../components/UploadLogs'
import { PlainLayout } from '../components/WizardLayout'
import * as ipc from '../lib/ipc'
import { useSelfCheck } from '../lib/useSelfCheck'
import type { OneClickFeedback, SelfCheckReview } from '../lib/types'
import { useStore } from '../state/context'
import type { ErrorCode } from '../state/machine'

/**
 * 通用错误页（技术方案第 18 节）。每个错误码一套标题 + 处理建议，
 * 下面永远有「一键反馈」（预填错误码）与「查看日志」两个出口。
 *
 * **I11 起这一页会自己去看现状**（U1 / U3）。0.1.9 在用户 Mac 上的那一幕：
 * 21:39 报错进这一页，22:05 外部把根因修好、六个服务全绿、网页 200，
 * 而这一页一直到 22:50 还挂着 21:39 那张卡片 —— 用户能做的只有点「重试」，
 * 点下去是重新下载 849 MB。
 *
 * 现在这一页做三件新的事：
 *   1. 低频复查（{@link useSelfCheck}），查到「已经在正常跑」就自己去运行面板；
 *   2. 复查结论常驻在错误卡片下面，AI / 规则层每跑完一个动作就重查一次；
 *   3. 多一个「关闭」—— 回托盘，不退出、不停容器。没异常时它就是主按钮。
 */
export function ErrorPage() {
  const { t, state, send, setOverlay, setNotice } = useStore()
  const check = useSelfCheck(state.name === 'Error')
  const review = check.review

  // **已经在正常跑了就别把人留在这一页。**
  // 判据是后端实测的「六个服务全就绪 + 本机网页真的应答」，不是我们猜的。
  useEffect(() => {
    if (!review || review.posture !== 'healthy') return
    setNotice(review.headline)
    send({ type: 'ALREADY_RUNNING' })
    // send / setNotice 引用稳定
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [review?.posture, review?.headline])

  if (state.name !== 'Error') return null

  const code = state.code as ErrorCode
  const info = t.error.codes[code] ?? t.error.codes.E_UNKNOWN
  // 容器全绿（哪怕网页那一下没探成）就说明这台机器上没有异常 ——
  // 这时候用户要的是「关掉它」，不是「再装一遍」
  const nothingWrong = !!review && review.ready === review.total && review.missing.length === 0

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
            {/* I11 · U3：关闭 = 回托盘。不退出进程、不动正在跑的服务。
                复查说没异常时它是主按钮 —— 那种情形下「重试」是错的那个选择。 */}
            <Button
              size="sm"
              variant={nothingWrong ? 'primary' : 'secondary'}
              data-testid="error-close"
              onClick={() => void ipc.windowHide()}
            >
              {t.error.close}
            </Button>
            <Button
              size="sm"
              variant={nothingWrong ? 'secondary' : 'primary'}
              data-testid="error-retry"
              onClick={() => send({ type: 'RETRY' })}
            >
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
          {/* I16 · 一键上传日志：脱敏后的日志直接进我们的排障库，用户只要念一个追踪码。
              和上面那条「发送诊断给开发者」的区别是：那一条要他自己去 GitHub 贴，
              这一条是他点一下就到我们手里 —— 客户一休息就断线的那个链条断在这里。 */}
          <UploadLogs stage={stageOf(state.from)} errorCode={code} summary={state.detail} />
          <SendToDev code={code} detail={state.detail} />
          <Button size="sm" onClick={() => setOverlay('feedback')}>
            {t.error.oneClickFeedback}
          </Button>
          {/* 方案 §18 给 E_PULL_FAILED 规定的动作就是「换源重试 / 离线导入」。
              换源由「重试」那条路自己做（flow::pull 会自动换源 3 次），这里补上离线导入。
              I16：拉着拉着不动了那一档同样该给这个出口 —— 两个源都不出数据时，
              离线包是唯一还走得通的路。 */}
          {(code === 'E_PULL_FAILED' || code === 'E_PULL_STALLED') && <OfflineImport variant="button" />}
        </div>
      </Card>

      {/* 复查结论（I11 · U1）。它就在失败卡片下面，用户不用去别处找 */}
      <NowCard className="mt-gap max-w-[820px]" check={check} />

      {/* 诊断助手（I4）：先跑确定性规则，认不出来才给「让 AI 帮我看看」。
          错误页是它最该出现的地方 —— 用户走到这里就是卡住了。

          `onActed`：AI 或规则层**跑完任何一个动作**就重查一次现状（I11 · U1）。
          0.1.9 那次，AI 读了四遍日志、规则层跑过若干动作，界面自始至终
          没有重新判断过「现在到底好了没有」。 */}
      <AssistPanel
        className="mt-gap max-w-[820px]"
        errorCode={code}
        errorMessage={state.detail}
        stage={state.from}
        onActed={check.refresh}
      />
    </PlainLayout>
  )
}

/**
 * 「现在是什么情况」卡片。
 *
 * 里面每一个数字都来自后端的实测（服务数、HTTP 状态码、耗时），
 * 拿不到的就写拿不到 —— 这一页最不该做的事就是让用户第二次误判现状。
 */
function NowCard({
  check,
  className = '',
}: {
  check: ReturnType<typeof useSelfCheck>
  className?: string
}) {
  const { t } = useStore()
  const [open, setOpen] = useState(false)
  const { review, loading, error, refresh } = check

  return (
    <Card className={className} data-testid="error-now">
      <div className="flex items-start gap-2.5">
        {loading ? (
          <Spinner size={14} className="mt-[3px] shrink-0 text-amber" />
        ) : review && review.ready === review.total && review.missing.length === 0 ? (
          <CheckCircle size={16} className="mt-[3px] shrink-0 text-success" />
        ) : (
          <AlertTriangle size={14} className="mt-[3px] shrink-0 text-muted" />
        )}
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-md font-medium text-ink">{t.error.nowTitle}</span>
            <span className="tnum shrink-0 rounded border border-line-strong px-1.5 py-[1px] text-xs text-muted">
              {t.error.nowBadge}
            </span>
          </div>
          <div className="mt-2 text-sm leading-[1.55] text-body">
            {loading
              ? t.error.nowChecking
              : error
                ? t.error.nowFailed(error)
                : review
                  ? review.headline
                  : t.error.nowChecking}
          </div>
          {review && (
            <div className="tnum mt-[6px] text-xs text-muted">
              {t.error.nowStat(review.ready, review.total, review.webStatus)}
            </div>
          )}
        </div>
      </div>

      <div className="mt-[14px] flex flex-wrap items-center gap-[10px] border-t border-line pt-[12px]">
        <Button size="sm" variant="ghost" data-testid="error-now-refresh" onClick={refresh}>
          {t.error.nowRefresh}
        </Button>
        {review && review.lines.length > 0 && (
          <Button size="sm" variant="ghost" onClick={() => setOpen((v) => !v)}>
            {open ? t.error.nowHide : t.error.nowShow}
          </Button>
        )}
        <span className="ml-auto text-xs text-muted">{t.error.nowAutoHint}</span>
      </div>

      {open && review && (
        <pre className="selectable tnum mt-[10px] max-h-[220px] overflow-auto whitespace-pre-wrap break-all rounded-md border border-line bg-log px-3 py-2.5 text-xs leading-[1.45] text-dim">
          {review.lines.join('\n')}
        </pre>
      )}
    </Card>
  )
}

/** 复查结论的只读快照类型，给测试与演示数据共用。 */
export type { SelfCheckReview }

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

/**
 * 出错时的「阶段」。上传日志时带上它，我们在库里一眼看得出这条记录是哪一步炸的。
 *
 * 用的是状态机里「从哪一页出的错」，不是我们另起一套名字 ——
 * 两套名字迟早会走岔。
 */
function stageOf(from: string): string {
  switch (from) {
    case 'AutoInstalling':
    case 'Pulling':
      return 'install'
    case 'Starting':
      return 'start'
    case 'Upgrading':
      return 'upgrade'
    default:
      return from.toLowerCase()
  }
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-4">
      <span className="w-[72px] shrink-0 text-md text-label">{label}</span>
      <span className="tnum selectable min-w-0 break-all text-md text-ink-2">{value}</span>
    </div>
  )
}
