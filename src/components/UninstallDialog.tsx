import { useEffect, useState } from 'react'
import { Button } from './Button'
import { Checkbox, TextInput } from './Field'
import { Modal } from './Modal'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { confirmOk } from '../lib/danger'
import { useStore } from '../state/context'
import type { UninstallPlan, UninstallReport, UninstallScope } from '../lib/types'

/**
 * 删除应用（I13 · R4）。用户 2026-09-22 的原话：
 *
 * > 可以执行删除应用，但需要输入「删除应用」文字，避免用户错误点击按钮。
 * > 删除前，还需要提醒，数据库是否需要保留，还是只删除应用。
 *
 * 三步：**选范围 → 逐字输入 → 执行并报告**。
 *
 * 三条界面上的硬规矩：
 *
 * 1. 默认选中的是后果最轻的那一项（「只删应用、保留数据」）；
 * 2. 「顺便删掉运行环境」这个勾选在「保留数据」那一档下是**禁用的** ——
 *    内置运行时下数据卷就在那台虚拟机的磁盘里，删了它等于把数据一起删掉。
 *    禁用的同时把原因写在旁边（`runtimeBlocked` 是后端算出来的，不是前端猜的）；
 * 3. 输入框差一个字，「确认删除」就一直是灰的。
 */
export function UninstallDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const { t } = useStore()
  const [scope, setScope] = useState<UninstallScope>(() =>
    ipc.demoPage() === 'uninstall-all' ? 'app-and-data' : 'app-only',
  )
  const plan = useAsync<UninstallPlan>(() => ipc.uninstallPlan(scope, scope === 'app-and-data'), [scope])
  const [removeImages, setRemoveImages] = useState(false)
  const [removeRuntime, setRemoveRuntime] = useState(false)
  const [backupFirst, setBackupFirst] = useState(() => ipc.demoPage() === 'uninstall-all')
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [steps, setSteps] = useState<string[]>([])
  const [report, setReport] = useState<UninstallReport | null>(null)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    let un: (() => void) | undefined
    void ipc.onUninstallStep((l) => setSteps((v) => [...v, l])).then((f) => {
      un = f
    })
    return () => un?.()
  }, [])

  /**
   * 换范围时把已经打好的字清掉 —— 两档要打的句子不一样，留着会造成「看起来输对了」。
   *
   * 这件事放在点击处理里做，不放 effect 里：在 effect 里同步 setState 会触发
   * 级联渲染（eslint 的 react-hooks/set-state-in-effect 正是盯这个）。
   */
  function pickScope(id: UninstallScope) {
    setScope(id)
    setTyped('')
    setRemoveRuntime(id === 'app-and-data')
    setBackupFirst(id === 'app-and-data')
  }

  const p = plan.data
  const phrase = p?.confirmPhrase ?? (scope === 'app-and-data' ? '删除应用和数据' : '删除应用')
  const runtimeBlocked = p?.runtimeBlocked ?? null

  async function go() {
    setBusy(true)
    setErr(null)
    setSteps([])
    try {
      setReport(
        await ipc.uninstallRun({
          scope,
          removeImages,
          removeRuntime: removeRuntime && !runtimeBlocked,
          backupFirst,
          confirm: typed,
        }),
      )
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  if (report) {
    return (
      <Modal
        testId="uninstall-done"
        title={t.uninstall.step3}
        footer={
          <Button size="sm" variant="primary" data-testid="uninstall-finish" onClick={onDone}>
            {t.uninstall.backToWelcome}
          </Button>
        }
      >
        <div className="tnum text-md text-ink">{report.headline}</div>
        <div className="mt-[14px] text-sm text-label">{t.uninstall.removed}</div>
        <ul className="mt-[6px] flex flex-col gap-[4px] text-xs leading-[1.5] text-body">
          {report.steps.map((l, i) => (
            <li key={`${i}-${l}`}>· {l}</li>
          ))}
        </ul>
        {report.kept.length > 0 && (
          <>
            <div className="mt-[14px] text-sm text-label">{t.uninstall.kept}</div>
            <ul className="mt-[6px] flex flex-col gap-[4px] text-xs leading-[1.5] text-body">
              {report.kept.map((l) => (
                <li key={l}>· {l}</li>
              ))}
            </ul>
          </>
        )}
        {report.failures.length > 0 && (
          <>
            <div className="mt-[14px] text-sm text-danger">{t.uninstall.failures}</div>
            <ul className="mt-[6px] flex flex-col gap-[4px] text-xs leading-[1.5] text-danger">
              {report.failures.map((l) => (
                <li key={l}>· {l}</li>
              ))}
            </ul>
          </>
        )}
        {scope === 'app-only' && (
          <div className="mt-[14px] text-xs leading-[1.5] text-muted">{t.uninstall.reinstallHint}</div>
        )}
      </Modal>
    )
  }

  return (
    <Modal
      testId="uninstall-dialog"
      title={t.uninstall.title}
      onClose={busy ? undefined : onClose}
      footer={
        <>
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>
            {t.common.cancel}
          </Button>
          <Button
            size="sm"
            variant="danger"
            data-testid="uninstall-go"
            disabled={busy || !confirmOk(typed, phrase)}
            onClick={() => void go()}
          >
            {busy ? t.uninstall.working : t.uninstall.go}
          </Button>
        </>
      }
    >
      <div className="text-sm text-label">{t.uninstall.step1}</div>
      <div className="mt-[10px] flex flex-col gap-[8px]">
        {(
          [
            ['app-only', t.uninstall.scopeAppOnly, t.uninstall.scopeAppOnlyHint, true],
            ['app-and-data', t.uninstall.scopeAll, t.uninstall.scopeAllHint, false],
          ] as const
        ).map(([id, label, hint, rec]) => (
          <button
            key={id}
            type="button"
            data-testid={`uninstall-scope-${id}`}
            onClick={() => pickScope(id)}
            className={`rounded-md border px-4 py-3 text-left transition-colors ${
              scope === id ? 'border-amber/60 bg-amber-soft' : 'border-line-strong bg-card hover:border-amber/40'
            }`}
          >
            <div className="flex items-center gap-2">
              <span className={`text-md ${scope === id ? 'text-amber-text' : 'text-ink'}`}>{label}</span>
              {rec && <span className="text-xs text-muted">（{t.uninstall.recommended}）</span>}
            </div>
            <div className="mt-[4px] text-xs leading-[1.5] text-muted">{hint}</div>
          </button>
        ))}
      </div>

      {/* 数据概况：范围 ② 的界面上必须摆出来（方案 R4 第 1 步） */}
      {scope === 'app-and-data' && (
        <div className="mt-[14px] rounded-md border border-danger/40 bg-danger-soft px-3 py-2.5 text-sm leading-[1.5] text-danger">
          {p?.tableCount != null && p.lastWrite
            ? t.uninstall.dataSummary(p.tableCount, p.lastWrite)
            : t.uninstall.dataSummaryUnknown}
          {p && p.backups === 0 && <div className="mt-[4px]">{t.uninstall.noBackup}</div>}
        </div>
      )}

      <div className="mt-[14px] flex flex-col gap-[10px]">
        {scope === 'app-and-data' && (
          <Checkbox on={backupFirst} onChange={setBackupFirst}>
            <span data-testid="uninstall-backup-first">{t.uninstall.optBackup}</span>
            <span className="mt-[2px] block text-xs leading-[1.5] text-muted">{t.uninstall.optBackupHint}</span>
          </Checkbox>
        )}
        <Checkbox on={removeImages} onChange={setRemoveImages}>
          <span data-testid="uninstall-images">
            {t.uninstall.optImages(p?.imagesBytes != null ? bytes(p.imagesBytes) : t.app.noData)}
          </span>
          <span className="mt-[2px] block text-xs leading-[1.5] text-muted">{t.uninstall.optImagesHint}</span>
        </Checkbox>
        {p?.builtinRuntime && (
          <Checkbox
            on={removeRuntime && !runtimeBlocked}
            disabled={!!runtimeBlocked}
            onChange={setRemoveRuntime}
          >
            <span data-testid="uninstall-runtime">
              {t.uninstall.optRuntime(p.runtimeBytes != null ? bytes(p.runtimeBytes) : t.app.noData)}
            </span>
            {runtimeBlocked && (
              <span
                className="mt-[2px] block text-xs leading-[1.5] text-amber-text"
                data-testid="uninstall-runtime-blocked"
              >
                {runtimeBlocked}
              </span>
            )}
          </Checkbox>
        )}
      </div>

      {p && (
        <div className="tnum mt-[14px] text-xs leading-[1.6] text-muted">
          {t.uninstall.willFree(bytes(p.estFreedBytes))} · {t.uninstall.keepBackupDir(p.backupDir)}
        </div>
      )}
      {/* I14 · F4：定时备份任务会跟着一起被收回 —— 说清楚它什么时候回来。
          0.1.13 真机验收时用户以为这一步是永久的，重装之后自己去手工装了一遍。 */}
      {p?.scheduleInstalled && (
        <div className="mt-[6px] text-xs leading-[1.5] text-muted" data-testid="uninstall-schedule-hint">
          {t.uninstall.scheduleHint}
        </div>
      )}
      {p?.warnings.map((w) => (
        <div key={w} className="mt-[6px] text-xs leading-[1.5] text-amber-text">
          {w}
        </div>
      ))}

      <div className="mt-[18px] text-sm text-label">{t.uninstall.step2}</div>
      <div className="mt-[8px] text-sm text-body">{t.uninstall.typeHint(phrase)}</div>
      <div className="mt-[8px]">
        <TextInput
          value={typed}
          onChange={setTyped}
          placeholder={t.uninstall.typePlaceholder}
          disabled={busy}
        />
      </div>

      {steps.length > 0 && (
        <ul className="mt-[12px] flex flex-col gap-[4px] text-xs leading-[1.5] text-body" data-testid="uninstall-steps">
          {steps.map((l, i) => (
            <li key={`${i}-${l}`}>· {l}</li>
          ))}
        </ul>
      )}
      {err && <div className="mt-[12px] text-sm leading-[1.5] text-danger">{err}</div>}
    </Modal>
  )
}
