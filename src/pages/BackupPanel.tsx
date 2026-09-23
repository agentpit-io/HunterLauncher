import { useEffect, useState } from 'react'
import { Badge } from '../components/Badge'
import { Button } from '../components/Button'
import { Card, CardHead } from '../components/Card'
import { Modal } from '../components/Modal'
import { TextInput } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { confirmOk } from '../lib/danger'
import { useStore } from '../state/context'
import type { BackupMeta, RestorePreflight, RestoreReport } from '../lib/types'

/**
 * 备份与恢复（I13 · R6 6.4）。
 *
 * 三件事在这一页上：**列出来 / 做一份 / 恢复一份**。
 *
 * 两个刻意的设计：
 *
 * 1. **每一份备份都摆出它的校验结论。** `pg_restore --list` 读不出目录的那一份
 *    压根不会被写成功（后端就判失败了），所以这里看到的「校验通过」是真的；
 *    0.1.12 及之前那些老备份没有这一项，界面上写「没有校验记录」而不是「通过」。
 * 2. **恢复要逐字输入「恢复数据」。** 和删除应用同一条原则 ——
 *    会覆盖现有数据的操作不该被一次误点触发。按钮在输对之前一直是灰的。
 */
export function BackupPanel() {
  const { t, setOverlay } = useStore()
  const [nonce, setNonce] = useState(0)
  const list = useAsync(() => ipc.listBackups(), [nonce])
  const settings = useAsync(() => ipc.readBackupSettings(), [nonce])
  const schedule = useAsync(() => ipc.backupScheduleStatus(), [nonce])
  const [busy, setBusy] = useState<'create' | 'restore' | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [steps, setSteps] = useState<string[]>([])
  const [picked, setPicked] = useState<BackupMeta | null>(null)
  const [pre, setPre] = useState<RestorePreflight | null>(null)
  const [typed, setTyped] = useState('')
  const [phrase, setPhrase] = useState('恢复数据')
  const [report, setReport] = useState<RestoreReport | null>(null)
  const [other, setOther] = useState<BackupMeta[] | null>(null)

  useEffect(() => {
    let un: (() => void) | undefined
    void ipc.onBackupStep((l) => setSteps((v) => [...v, l])).then((f) => {
      un = f
    })
    void ipc.restoreConfirmText().then(setPhrase).catch(() => {})
    // 截图脚本用 HUNTER_DEMO_PAGE=backup-restore 直接把恢复确认那张弹窗打开
    if (ipc.demoPage() === 'backup-restore') {
      void ipc
        .listBackups()
        .then((v) => v[0] && void openRestore(v[0]))
        .catch(() => {})
    }
    return () => un?.()
  }, [])

  async function onCreate() {
    setBusy('create')
    setNote(null)
    setSteps([])
    try {
      const m = await ipc.createBackup()
      setNote(
        `${m.id} · ${bytes(m.totalBytes)} · ${m.verified ? t.backup.verified : t.backup.notVerified}`,
      )
      setNonce((n) => n + 1)
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  async function openRestore(m: BackupMeta) {
    setPicked(m)
    setTyped('')
    setReport(null)
    setPre(null)
    try {
      setPre(await ipc.restorePreflight(m.dir || m.id))
    } catch (e) {
      setPre({
        id: m.id,
        dir: m.dir,
        tag: m.tag,
        currentTag: '',
        needsUpgrade: false,
        hasDump: false,
        legacySql: false,
        verified: false,
        checksumMismatch: [],
        volumes: [],
        blocked: e instanceof Error ? e.message : String(e),
        lines: [],
      })
    }
  }

  async function onRestore() {
    if (!picked) return
    setBusy('restore')
    setSteps([])
    try {
      setReport(await ipc.restoreBackup(picked.dir || picked.id, typed))
      setNonce((n) => n + 1)
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
      setPicked(null)
    } finally {
      setBusy(null)
    }
  }

  async function onPickOther() {
    try {
      const d = await ipc.pickBackupDir()
      if (!d) return
      setOther(await ipc.backupsInDir(d))
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    }
  }

  const s = settings.data
  const sc = schedule.data
  const items = other ?? list.data ?? []

  return (
    <PlainLayout
      title={t.backup.title}
      right={
        <>
          <Button size="sm" data-testid="backup-create" disabled={busy !== null} onClick={() => void onCreate()}>
            {busy === 'create' ? t.backup.creating : t.backup.create}
          </Button>
          <Button size="sm" onClick={() => setOverlay(null)}>
            {t.common.close}
          </Button>
        </>
      }
    >
      <p className="max-w-[860px] text-sm leading-[1.6] text-muted">{t.backup.intro}</p>

      <div className="mt-[18px] grid grid-cols-2 gap-gap">
        <Card>
          <CardHead title={t.backup.dir} />
          <div className="tnum mt-[12px] break-all text-md text-ink">
            {s?.effectiveDir ?? t.app.noData}
          </div>
          <div className="tnum mt-[8px] text-sm text-muted">
            {s ? t.backup.count(s.count, bytes(s.totalBytes)) : t.app.noData}
            {s?.diskFreeBytes != null && ` · ${t.backup.diskFree(bytes(s.diskFreeBytes))}`}
          </div>
          <div className="mt-[14px] flex gap-[10px]">
            <Button size="sm" onClick={() => setOverlay('settings')}>
              {t.backup.changeDir}
            </Button>
            <Button size="sm" data-testid="backup-from-other" onClick={() => void onPickOther()}>
              {t.backup.fromOther}
            </Button>
          </div>
          <div className="mt-[10px] text-xs leading-[1.5] text-muted">{t.backup.fromOtherHint}</div>
        </Card>

        <Card>
          <CardHead title={t.backup.scheduleTitle} />
          <div className="mt-[12px] text-md text-ink">
            {sc?.wanted ? t.backup.scheduleOn(sc.time) : t.backup.scheduleOff}
          </div>
          <div className="mt-[8px] flex flex-col gap-[4px] text-sm text-muted">
            {sc && <div>{t.backup.scheduleMech(sc.mech)}</div>}
            {sc && !sc.supported && <div className="text-amber-text">{t.backup.scheduleUnsupported}</div>}
            {sc?.supported && !sc.installed && sc.wanted && (
              <div className="text-amber-text">{t.backup.scheduleNotInstalled}</div>
            )}
            {sc?.nextRun && <div className="tnum break-all">{t.backup.scheduleNext(sc.nextRun)}</div>}
            {sc?.lines.map((l) => (
              <div key={l} className="break-all">
                · {l}
              </div>
            ))}
            {sc?.reason && <div className="text-amber-text">{sc.reason}</div>}
          </div>
        </Card>
      </div>

      {note && (
        <div className="mt-[14px] rounded-md border border-line bg-card px-4 py-2.5 text-sm leading-[1.5] text-amber-text">
          {note}
        </div>
      )}
      {busy === 'create' && steps.length > 0 && (
        <ul className="mt-[10px] flex flex-col gap-[4px] text-xs text-body" data-testid="backup-steps">
          {steps.map((l, i) => (
            <li key={`${i}-${l}`}>· {l}</li>
          ))}
        </ul>
      )}

      <div className="mt-[18px] flex flex-col gap-[10px]" data-testid="backup-list">
        {items.length === 0 && <div className="text-sm text-muted">{t.backup.empty}</div>}
        {items.map((m) => (
          <Card key={m.dir || m.id} className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-[10px]">
                <span className="tnum text-md text-ink">{m.at}</span>
                <Badge tone="neutral">v{m.tag}</Badge>
                <Badge tone="neutral">{kindLabel(m, t)}</Badge>
                {m.verified ? (
                  <Badge tone="success">{t.backup.verified}</Badge>
                ) : (
                  <Badge tone="neutral">{m.sqlBytes ? t.backup.legacy : t.backup.notVerified}</Badge>
                )}
              </div>
              <div className="tnum mt-[6px] break-all text-xs text-muted">{m.dir || m.id}</div>
              <div className="tnum mt-[6px] text-sm text-body">
                {bytes(m.totalBytes || m.dumpBytes || m.sqlBytes || 0)}
                {m.tableCount != null && ` · ${t.backup.tables(m.tableCount, m.rowsTotal ?? 0)}`}
                {m.volumes.filter((v) => v.bytes != null).length > 0 &&
                  ` · ${t.backup.volumes} ${m.volumes
                    .filter((v) => v.bytes != null)
                    .map((v) => v.short)
                    .join('、')}`}
              </div>
              {m.verifyNote && <div className="mt-[4px] text-xs text-muted">{m.verifyNote}</div>}
              {m.dumpError && <div className="mt-[4px] text-xs text-danger">{m.dumpError}</div>}
            </div>
            <Button
              size="sm"
              data-testid={`restore-${m.id}`}
              onClick={() => void openRestore(m)}
            >
              {t.backup.restore}
            </Button>
          </Card>
        ))}
      </div>

      {picked && !report && (
        <Modal
          testId="restore-dialog"
          title={t.backup.restoreTitle}
          onClose={busy === null ? () => setPicked(null) : undefined}
          footer={
            <>
              <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => setPicked(null)}>
                {t.common.cancel}
              </Button>
              <Button
                size="sm"
                variant="primary"
                data-testid="restore-go"
                disabled={busy !== null || !confirmOk(typed, phrase) || !!pre?.blocked}
                onClick={() => void onRestore()}
              >
                {busy === 'restore' ? t.backup.restoring : t.backup.restoreGo}
              </Button>
            </>
          }
        >
          <div className="tnum text-sm text-muted">{picked.at} · v{picked.tag}</div>
          <p className="mt-[10px] text-sm leading-[1.6] text-body">{t.backup.restoreBody}</p>
          <p className="mt-[10px] text-xs leading-[1.6] text-muted">{t.backup.restoreSteps}</p>
          {pre?.lines.map((l) => (
            <div key={l} className="mt-[6px] text-xs leading-[1.5] text-muted">
              · {l}
            </div>
          ))}
          {pre?.blocked && (
            <div className="mt-[12px] rounded-md border border-danger/40 bg-danger-soft px-3 py-2 text-sm leading-[1.5] text-danger">
              {pre.blocked}
            </div>
          )}
          {!pre?.blocked && (
            <div className="mt-[16px]">
              <div className="text-sm text-label">{t.backup.restoreType(phrase)}</div>
              <div className="mt-[8px]">
                <TextInput value={typed} onChange={setTyped} placeholder={phrase} disabled={busy !== null} />
              </div>
            </div>
          )}
          {steps.length > 0 && (
            <ul className="mt-[12px] flex flex-col gap-[4px] text-xs text-body">
              {steps.map((l, i) => (
                <li key={`${i}-${l}`}>· {l}</li>
              ))}
            </ul>
          )}
        </Modal>
      )}

      {report && (
        <Modal
          testId="restore-done"
          title={t.backup.restoreDone}
          onClose={() => {
            setReport(null)
            setPicked(null)
          }}
          footer={
            <Button
              size="sm"
              variant="primary"
              onClick={() => {
                setReport(null)
                setPicked(null)
              }}
            >
              {t.common.gotIt}
            </Button>
          }
        >
          <div className="tnum text-md text-ink">{report.headline}</div>
          {report.safetyBackup && (
            <div className="mt-[8px] text-sm text-muted">{t.backup.safetyBackup(report.safetyBackup)}</div>
          )}
          <ul className="mt-[12px] flex flex-col gap-[4px] text-xs leading-[1.5] text-body">
            {report.steps.map((l, i) => (
              <li key={`${i}-${l}`}>· {l}</li>
            ))}
          </ul>
        </Modal>
      )}
    </PlainLayout>
  )
}

function kindLabel(m: BackupMeta, t: ReturnType<typeof useStore>['t']): string {
  if (m.kind === 'scheduled') return t.backup.kindScheduled
  if (m.kind === 'manual') return t.backup.kindManual
  return t.backup.kindPreUpgrade
}
