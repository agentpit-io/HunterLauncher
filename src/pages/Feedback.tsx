import { useState } from 'react'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { Checkbox, Field, SegmentedControl, TextArea, TextInput } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'

type FeedbackType = 'deploy' | 'result' | 'feature' | 'other'

/**
 * 反馈页（技术方案 §11.1 的启动器侧）。
 *
 * M0 结论：`telemetry.agentpit.io` 不存在，上游 api 也没有 `/api/feedback`。
 * 所以这一页**不会自动发送任何东西**，只做三件真的做得到的事：
 *
 * 1. 把诊断信息**逐节**摆出来（每一节都已经脱敏），用户可以逐节勾掉不带；
 * 2. 导出成一个 zip 放在 `~/.hunter/diagnostics/`；
 * 3. 打开一个预填好标题与正文的 GitHub issue —— **诊断包不自动上传**。
 *
 * 界面底部常驻「不会自动上传」，不做「提交成功」的假象（总控规则红线 1）。
 */
export function Feedback({ errorCode }: { errorCode?: string }) {
  const { t, setOverlay } = useStore()
  const [type, setType] = useState<FeedbackType>(errorCode ? 'deploy' : 'other')
  const [desc, setDesc] = useState('')
  const [contact, setContact] = useState('')
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState<'zip' | 'issue' | null>(null)
  const [expanded, setExpanded] = useState<string | null>(null)
  /** 哪几节带上。null = 还没拿到诊断，拿到之后按每节的 defaultOn 初始化 */
  const [include, setInclude] = useState<Record<string, boolean> | null>(null)

  const diag = useAsync(() => ipc.diagnosticsSections(), [])
  const sections = diag.data ?? []
  if (sections.length > 0 && include === null) {
    setInclude(Object.fromEntries(sections.map((s) => [s.id, s.defaultOn && s.body.length > 0])))
  }
  const on = (id: string) => include?.[id] ?? false
  const chosen = sections.filter((s) => on(s.id)).map((s) => s.id)

  const form = () => ({ kind: type, description: desc, contact, errorCode: errorCode ?? '' })

  async function doExport() {
    setBusy('zip')
    setNote(null)
    try {
      const r = await ipc.exportDiagnostics(chosen, form())
      setNote(t.feedback.exportOk(r.path, `${(r.bytes / 1024).toFixed(1)} KB`))
    } catch (e) {
      setNote(t.feedback.exportFail(e instanceof Error ? e.message : String(e)))
    } finally {
      setBusy(null)
    }
  }

  async function doIssue() {
    if (!desc.trim()) {
      setNote(t.feedback.descRequired)
      return
    }
    setBusy('issue')
    setNote(null)
    try {
      await ipc.openExternal(await ipc.feedbackIssueUrl(form()))
      setNote(t.feedback.issueHint)
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <PlainLayout
      title={t.feedback.title}
      right={<Button size="sm" onClick={() => setOverlay(null)}>{t.common.close}</Button>}
      footer={
        <>
          <span className="min-w-0 truncate text-sm text-muted" data-testid="feedback-note">
            {note ?? t.feedback.noUpload}
          </span>
          <div className="flex gap-[10px]">
            <Button
              size="sm"
              data-testid="feedback-export"
              disabled={busy !== null}
              onClick={() => void doExport()}
            >
              {busy === 'zip' ? t.common.working : t.feedback.exportBundle}
            </Button>
            <Button
              size="sm"
              variant="primary"
              data-testid="feedback-issue"
              disabled={busy !== null}
              onClick={() => void doIssue()}
            >
              {busy === 'issue' ? t.common.working : t.feedback.openIssue}
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
              testIdPrefix="feedback-type"
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
          <Field label={t.feedback.contactLabel} hint={t.feedback.attachHint}>
            <TextInput value={contact} onChange={setContact} placeholder="optional@example.com" />
          </Field>
          {errorCode && (
            <div className="tnum rounded-md border border-line bg-log px-4 py-3 text-sm text-dim">
              error_code = {errorCode}
            </div>
          )}
        </div>

        {/* 诊断信息预览与删减（方案 §11.1「用户可预览与删除」） */}
        <Card className="flex min-h-0 flex-col">
          <div className="text-md font-medium text-ink">{t.feedback.sectionsTitle}</div>
          <div className="mt-[6px] text-xs leading-[1.6] text-muted">{t.feedback.redactNote}</div>

          {diag.loading && <div className="mt-[14px] text-sm text-muted">{t.common.loading}</div>}
          {diag.error && (
            <div className="mt-[14px] text-sm text-danger">
              {diag.error.code} · {diag.error.message}
            </div>
          )}

          <div className="mt-[14px] flex flex-col gap-[10px]">
            {sections.map((sec) => (
              <div key={sec.id} className="rounded-md border border-line bg-log px-3 py-2.5">
                <div className="flex items-start justify-between gap-3">
                  <Checkbox
                    on={on(sec.id)}
                    onChange={(v) => setInclude({ ...(include ?? {}), [sec.id]: v })}
                  >
                    <span data-testid={`diag-${sec.id}`} className="text-sm text-ink-2">
                      {sec.title}
                    </span>
                  </Checkbox>
                  {sec.body.length > 0 && (
                    <button
                      type="button"
                      data-testid={`diag-toggle-${sec.id}`}
                      onClick={() => setExpanded(expanded === sec.id ? null : sec.id)}
                      className="shrink-0 text-xs text-muted transition-colors hover:text-amber-text"
                    >
                      {expanded === sec.id ? t.feedback.collapse : t.feedback.expand}
                    </button>
                  )}
                </div>
                {sec.note && (
                  <div className="mt-[6px] text-xs leading-[1.5] text-muted">{sec.note}</div>
                )}
                {expanded === sec.id && (
                  <pre className="tnum selectable mt-[8px] max-h-[220px] overflow-auto whitespace-pre-wrap break-all text-xs leading-[1.5] text-dim">
                    {sec.body || t.feedback.sectionEmpty}
                  </pre>
                )}
              </div>
            ))}
          </div>
        </Card>
      </div>
    </PlainLayout>
  )
}
