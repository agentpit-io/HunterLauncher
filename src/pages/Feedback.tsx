import { useState } from 'react'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { Checkbox, Field, SegmentedControl, TextArea, TextInput } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'

type FeedbackType = 'deploy' | 'result' | 'feature' | 'other'
const ISSUE_URL = 'https://github.com/agentpit-io/HunterLauncher/issues/new'

/**
 * 反馈页。
 * M0 结论：telemetry.agentpit.io 不存在，上游 api 也没有 /api/feedback。
 * 所以这一页**不会自动发送任何东西**，只做两件事：导出脱敏诊断包、打开预填好的 GitHub issue。
 * 界面上把这一点写在最显眼的地方，不做「提交成功」的假象（总控规则红线 1）。
 */
export function Feedback({ errorCode }: { errorCode?: string }) {
  const { t, setOverlay } = useStore()
  const [type, setType] = useState<FeedbackType>(errorCode ? 'deploy' : 'other')
  const [desc, setDesc] = useState('')
  const [contact, setContact] = useState('')
  const [attach, setAttach] = useState(true)

  return (
    <PlainLayout
      title={t.feedback.title}
      right={<Button size="sm" onClick={() => setOverlay(null)}>{t.common.close}</Button>}
      footer={
        <>
          <span className="text-sm text-muted">{t.feedback.noUpload}</span>
          <div className="flex gap-[10px]">
            <Button size="sm">{t.feedback.exportBundle}</Button>
            <Button
              size="sm"
              variant="primary"
              onClick={() => void ipc.openExternal(`${ISSUE_URL}?title=${encodeURIComponent(errorCode ?? '')}`)}
            >
              {t.feedback.openIssue}
            </Button>
          </div>
        </>
      }
    >
      <p className="max-w-[760px] text-md leading-[1.55] text-body">{t.feedback.intro}</p>

      <div className="mt-[24px] grid grid-cols-2 gap-gap">
        <div className="flex flex-col gap-[20px]">
          <Field label={t.feedback.typeLabel}>
            <SegmentedControl<FeedbackType>
              value={type}
              onChange={setType}
              options={[
                { id: 'deploy', label: t.feedback.types.deploy },
                { id: 'result', label: t.feedback.types.result },
                { id: 'feature', label: t.feedback.types.feature },
                { id: 'other', label: t.feedback.types.other },
              ]}
            />
          </Field>
          <Field label={t.feedback.descLabel}>
            <TextArea value={desc} onChange={setDesc} placeholder={t.feedback.descPlaceholder} rows={7} />
          </Field>
          <Field label={t.feedback.contactLabel}>
            <TextInput value={contact} onChange={setContact} placeholder="optional@example.com" />
          </Field>
        </div>

        <Card className="flex flex-col">
          <Checkbox on={attach} onChange={setAttach}>
            {t.feedback.attachLabel}
          </Checkbox>
          <div className="mt-[12px] text-xs leading-[1.6] text-muted">{t.feedback.attachHint}</div>
          {errorCode && (
            <div className="tnum mt-[16px] rounded-md border border-line bg-log px-4 py-3 text-sm text-dim">
              error_code = {errorCode}
            </div>
          )}
          <div className="mt-auto pt-4">
            <Button size="sm">{t.feedback.preview}</Button>
          </div>
        </Card>
      </div>
    </PlainLayout>
  )
}
