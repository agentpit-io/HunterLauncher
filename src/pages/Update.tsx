import { useEffect, useRef, useState } from 'react'
import { Badge } from '../components/Badge'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { LogBox } from '../components/LogBox'
import { Modal } from '../components/Modal'
import { PlainLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import { isoToShanghai } from '../lib/format'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { useStore } from '../state/context'
import type { BackupMeta, LauncherUpdate, UpgradeStatus } from '../lib/types'

/**
 * 更新页（技术方案 §5.6、§10）。视觉稿里没有这一页，按同一套设计令牌延展：
 * 左右两栏卡片、等宽字体显示版本号与路径、金色只用在"要用户决定的那一下"。
 *
 * 三块内容：
 *
 * 1. **启动器自更新** —— 能就地装的（AppImage / Windows / macOS）点一下装完重启；
 *    `.deb` 只下载 + 给一条命令（原因写在卡片里，不含糊）。
 * 2. **Hunter 升级** —— Release Notes 摘要 + 跨大版本提示 + 升级按钮 + 实时步骤。
 *    升级前的备份、失败回滚的行为都写在按钮旁边，点之前就看得到。
 * 3. **备份** —— 列 `~/.hunter/backups/`，可以手动做一份，也可以从某一份恢复数据库。
 *
 * 红线 1：查不到就显示原因，不写"已经是最新版"。
 */
export function Update() {
  const { t, setOverlay } = useStore()

  return (
    <PlainLayout
      title={t.update.hunterCheck}
      right={
        <Button size="sm" data-testid="update-close" onClick={() => setOverlay(null)}>
          {t.common.close}
        </Button>
      }
    >
      <div className="grid grid-cols-2 gap-gap">
        <LauncherCard />
        <HunterCard />
        <BackupsCard />
      </div>
    </PlainLayout>
  )
}

// ── 启动器自更新 ─────────────────────────────────────────────────────────

function LauncherCard() {
  const { t } = useStore()
  const u = useAsync(() => ipc.checkLauncherUpdate(), [])
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const d: LauncherUpdate | null = u.data ?? null

  async function install() {
    setBusy(true)
    setErr(null)
    try {
      // 这个 Promise 永远不会 resolve —— Rust 那边装完直接重启进程了。
      // I8：`.deb` 也走这条路（先弹 polkit 的原生授权框，再 dpkg -i），
      // 所以界面上不再有「复制命令自己去装」那条出口。
      await ipc.installLauncherUpdate()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card className="flex flex-col">
      <div className="flex items-center justify-between">
        {/* 标题跟着状态走：原来无论什么状态都写「启动器有新版本」，
            而正下方那一行写的是「启动器已经是最新版」—— 同一张卡片自己打自己（I3 自审）。 */}
        <div className="text-md font-medium text-ink">
          {d?.available ? t.update.launcherTitle : t.update.launcherCard}
        </div>
        {d?.available && <Badge tone="amber">v{d.version}</Badge>}
      </div>

      <div className="tnum mt-[14px] text-md text-body" data-testid="launcher-version">
        {u.loading
          ? t.common.loading
          : d?.available && d.version
            ? t.update.launcherLine(d.current, d.version)
            : d?.reason
              ? t.update.launcherFail(d.reason)
              : d
                ? `${t.update.launcherNone} · v${d.current}`
                : (u.error?.message ?? t.app.noDataReason)}
      </div>

      {d?.date && (
        <div className="mt-[6px] text-xs text-muted">{t.update.published(isoToShanghai(d.date))}</div>
      )}

      {d?.notes && (
        <pre className="selectable mt-[14px] max-h-[160px] overflow-auto whitespace-pre-wrap break-words rounded-md border border-line bg-log px-3 py-2.5 text-sm leading-[1.6] text-dim">
          {d.notes}
        </pre>
      )}

      {/* .deb 这一路要先把「为什么会弹系统密码框」讲清楚 —— 弹框之前，不是弹框之后 */}
      {d?.available && !d.canSelfInstall && (
        <div className="mt-[14px] rounded-md border border-line bg-window px-3 py-2.5 text-xs leading-[1.6] text-muted">
          {t.update.manualWhy}
        </div>
      )}

      {err && <div className="mt-[12px] text-sm leading-[1.5] text-danger">{err}</div>}

      <div className="mt-auto flex flex-wrap gap-[10px] pt-[16px]">
        <Button size="sm" disabled={u.loading} onClick={() => u.reload()}>
          {u.loading ? t.update.hunterChecking : t.update.hunterCheck}
        </Button>
        {d?.available && (
          <>
            <Button
              size="sm"
              variant="primary"
              data-testid="launcher-update-install"
              disabled={busy}
              onClick={() => void install()}
            >
              {busy ? t.update.launcherWorking : t.update.launcherNow}
            </Button>
            {d.version && (
              <Button
                size="sm"
                onClick={() =>
                  void ipc.openExternal(
                    `https://github.com/agentpit-io/HunterLauncher/releases/tag/launcher-v${d.version}`,
                  )
                }
              >
                {t.update.launcherRelease}
              </Button>
            )}
          </>
        )}
      </div>

    </Card>
  )
}


// ── Hunter 升级 ──────────────────────────────────────────────────────────

function HunterCard() {
  const { t } = useStore()
  const [nonce, setNonce] = useState(0)
  const c = useAsync(() => ipc.checkHunterUpdate(nonce > 0), [nonce])
  const [confirm, setConfirm] = useState(false)
  const [st, setSt] = useState<UpgradeStatus | null>(null)
  const timer = useRef<number | undefined>(undefined)
  const d = c.data ?? null

  // 升级要几分钟。开始之后每秒问一次进度，做完就停。
  useEffect(() => {
    return () => window.clearInterval(timer.current)
  }, [])

  function poll() {
    window.clearInterval(timer.current)
    timer.current = window.setInterval(() => {
      void ipc.upgradeStatus().then((s) => {
        setSt(s)
        if (!s.running) {
          window.clearInterval(timer.current)
          setNonce((n) => n + 1)
        }
      })
    }, 1000)
  }

  async function start(tag: string) {
    setConfirm(false)
    setSt({ running: true, steps: [], result: null, error: null })
    try {
      await ipc.upgradeHunter(tag)
      poll()
    } catch (e) {
      setSt({
        running: false,
        steps: [],
        result: null,
        error: e instanceof Error ? e.message : String(e),
      })
    }
  }

  const running = st?.running ?? false

  return (
    <Card className="flex flex-col">
      <div className="flex items-center justify-between">
        <div className="text-md font-medium text-ink">
          {d?.hasUpdate ? t.update.hunterTitle : t.update.hunterCard}
        </div>
        {d?.hasUpdate && d.latest && <Badge tone="amber">v{d.latest}</Badge>}
      </div>

      <div className="tnum mt-[14px] text-md text-body" data-testid="hunter-version">
        {c.loading
          ? t.common.loading
          : d?.hasUpdate && d.latest
            ? `${t.update.hunterCurrent(d.current)} · ${t.update.hunterLatest(d.latest)}`
            : d?.reason
              ? t.update.hunterFail(d.reason)
              : d
                ? t.update.hunterNone(d.current)
                : (c.error?.message ?? t.app.noDataReason)}
      </div>
      {d?.publishedAt && (
        <div className="mt-[6px] text-xs text-muted">
          {t.update.published(isoToShanghai(d.publishedAt))}
        </div>
      )}

      {d?.majorJump && d.hasUpdate && (
        <div className="mt-[12px] rounded-md border border-amber/40 bg-amber-soft px-3 py-2.5 text-xs leading-[1.6] text-amber-text">
          {t.update.hunterMajor}
        </div>
      )}

      {d?.notes && (
        <>
          <div className="mt-[14px] text-sm text-label">{t.update.hunterNotes}</div>
          <pre
            data-testid="hunter-notes"
            className="selectable mt-[8px] max-h-[180px] overflow-auto whitespace-pre-wrap break-words rounded-md border border-line bg-log px-3 py-2.5 text-sm leading-[1.6] text-dim"
          >
            {d.notes}
          </pre>
        </>
      )}

      {/* 升级过程 */}
      {st && (
        <>
          <div className="mt-[14px] flex items-center justify-between">
            <span className="text-sm text-label">{t.update.stepsTitle}</span>
            <span className="text-xs text-muted">
              {running
                ? t.update.hunterUpgrading
                : st.error
                  ? t.update.failed
                  : st.result
                    ? t.update.succeeded
                    : ''}
            </span>
          </div>
          <LogBox lines={st.steps} className="mt-[8px] max-h-[160px]" emptyText={t.common.loading} />
          {st.error && (
            <div className="mt-[10px] text-sm leading-[1.6] text-danger" data-testid="upgrade-error">
              {st.error}
            </div>
          )}
          {st.result && (
            <div className="mt-[10px] text-sm leading-[1.6] text-amber-text" data-testid="upgrade-result">
              {st.result.message}
            </div>
          )}
        </>
      )}

      <div className="mt-auto flex flex-wrap gap-[10px] pt-[16px]">
        <Button size="sm" disabled={c.loading || running} onClick={() => setNonce((n) => n + 1)}>
          {c.loading ? t.update.hunterChecking : t.update.hunterCheck}
        </Button>
        {d?.hasUpdate && d.latest && (
          <Button
            size="sm"
            variant="primary"
            data-testid="hunter-upgrade"
            disabled={running}
            onClick={() => setConfirm(true)}
          >
            {running ? t.update.hunterUpgrading : t.update.hunterUpgrade}
          </Button>
        )}
        {d?.notesUrl && (
          <Button size="sm" onClick={() => void ipc.openExternal(d.notesUrl!)}>
            {t.update.hunterNotesFull}
          </Button>
        )}
      </div>

      {confirm && d?.latest && (
        <Modal
          testId="upgrade-confirm"
          title={`${t.update.hunterUpgrade} v${d.current} → v${d.latest}`}
          onClose={() => setConfirm(false)}
          footer={
            <>
              <Button size="sm" variant="ghost" onClick={() => setConfirm(false)}>
                {t.common.cancel}
              </Button>
              <Button
                size="sm"
                variant="primary"
                data-testid="upgrade-go"
                onClick={() => void start(d.latest!)}
              >
                {t.update.hunterUpgrade}
              </Button>
            </>
          }
        >
          <p className="leading-[1.6]">{t.update.backupNote('~/.hunter/backups/')}</p>
          <p className="mt-[12px] leading-[1.6] text-muted">{t.update.rollbackNote}</p>
        </Modal>
      )}
    </Card>
  )
}

// ── 备份 ─────────────────────────────────────────────────────────────────
//
// I13 起**恢复不在这一页做了**：它要逐字输入「恢复数据」、要先给当前数据做一份
// 保命备份、要停服务再起回来 —— 那是「备份与恢复」那一页（`BackupPanel`）的事。
// 这里只留清单与「现在备份一份」，「从这份恢复」按钮改成跳过去。
// 留一个不做二次确认的恢复入口，等于在旁边开了一扇绕过那道门的小窗。

/** 这一份备份里数据库那一半有多大。新格式看 `dumpBytes`，老备份看 `sqlBytes`。 */
function dumpBytes(m: BackupMeta): number {
  return m.dumpBytes ?? m.sqlBytes ?? 0
}

function BackupsCard() {
  const { t, setOverlay } = useStore()
  const [nonce, setNonce] = useState(0)
  const b = useAsync(() => ipc.listBackups(), [nonce])
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const list = b.data ?? []

  async function makeOne() {
    setBusy(true)
    setNote(null)
    try {
      const m = await ipc.createBackup()
      setNote(t.update.backupRow(m.at, m.tag, dumpBytes(m) > 0 ? bytes(dumpBytes(m)) : '—'))
      setNonce((n) => n + 1)
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card className="col-span-2 flex flex-col">
      <div className="text-md font-medium text-ink">{t.update.backupsTitle}</div>
      <div className="mt-[8px] text-xs leading-[1.6] text-muted">{t.update.backupsHint}</div>

      <ul className="tnum mt-[14px] flex flex-col gap-[8px] text-sm text-dim" data-testid="backup-list">
        {list.length === 0 && <li className="text-muted">{t.update.backupsEmpty}</li>}
        {list.map((m) => (
          <li key={m.id} className="flex items-center justify-between gap-4 border-b border-line pb-[8px]">
            <span className="min-w-0 truncate">
              {t.update.backupRow(m.at, m.tag, dumpBytes(m) > 0 ? bytes(dumpBytes(m)) : '—')}
              {dumpBytes(m) === 0 && (
                <span className="ml-2 text-danger">
                  {t.update.backupNoDump}
                  {m.dumpError || m.sqlError ? ` · ${m.dumpError ?? m.sqlError}` : ''}
                </span>
              )}
            </span>
            {dumpBytes(m) > 0 && (
              <Button size="sm" onClick={() => setOverlay('backup')}>
                {t.update.backupRestore}
              </Button>
            )}
          </li>
        ))}
      </ul>

      {note && <div className="mt-[12px] text-sm leading-[1.6] text-amber-text">{note}</div>}

      <div className="mt-[16px] flex gap-[10px]">
        <Button size="sm" data-testid="backup-now" disabled={busy} onClick={() => void makeOne()}>
          {busy ? t.update.backupWorking : t.update.backupNow}
        </Button>
      </div>

    </Card>
  )
}
