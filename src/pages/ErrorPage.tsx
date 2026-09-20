import { AssistPanel } from '../components/AssistPanel'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { AlertTriangle } from '../components/Icons'
import { OfflineImport } from '../components/OfflineImport'
import { PlainLayout } from '../components/WizardLayout'
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
          <Button size="sm" variant="secondary" onClick={() => setOverlay('feedback')}>
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

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-4">
      <span className="w-[72px] shrink-0 text-md text-label">{label}</span>
      <span className="tnum selectable min-w-0 break-all text-md text-ink-2">{value}</span>
    </div>
  )
}
