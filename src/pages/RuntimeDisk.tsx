import { useState } from 'react'
import { Button, ChevronRight } from '../components/Button'
import { Card } from '../components/Card'
import { AlertTriangle, Spinner } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'
import {
  approxGb,
  canChangeDisk,
  chosenVolume,
  currentDir,
  freeGb,
  largestNonSystem,
  shortfall,
  systemVolume,
  usedState,
} from '../lib/disk'
import type { DiskPlan, DiskVolume } from '../lib/types'

/**
 * 装到哪块盘（R5 · U-03，需求 S-01 那套打分的**消费点**）。
 *
 * ## 这一页要回答四件事
 *
 * 默认位置、为什么是它、要占多少、现在还剩多少 —— 顺序就是用户心里的问题顺序。
 *
 * ## 数字一个都不在界面里产生
 *
 * 容量、剩余、要占多少、上限、实占**全部**来自 Rust 的
 * `runtime_disk_plan`（真实探测，或 `runtime/disk.rs` 里的常量）。
 * 这一页只做换算与拼句子，见 `src/lib/disk.ts`。
 *
 * ## 「上限」与「实占」**永远是两行**
 *
 * I14·F2 把稀疏文件报成「86.1 GB」而实际只有 5.8 GB（差 15 倍），
 * 所以这两个数在 Rust 那边就是两个字段、在界面上也必须落在两行里。
 * 量不到就写「量不到」——**不拿宿主上的文件大小顶替，也不编一个 0**。
 *
 * ## 全不合格时不许静默失败
 *
 * `chosen === null` 是 S-01 的正常返回值，不是异常。它必须变成一句
 * 用户看得懂的话：没有合适的盘、还差多少空间。
 */
export function RuntimeDisk() {
  const { t, send } = useStore()
  const plan = useAsync(() => ipc.runtimeDiskPlan(), [])
  const [busy, setBusy] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const p = plan.data

  /** 把选中的盘真的写进 `[runtime] data_dir`（走 R4 那条配置路径，没有第二条）。 */
  async function choose(mount: string) {
    setBusy(mount)
    setErr(null)
    try {
      await ipc.runtimeDiskSet(mount)
      send({ type: 'RUNTIME_DISK_CHOSEN' })
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
      setBusy(null)
    }
  }

  const chosen = p ? chosenVolume(p) : null
  // 用户手动换过的那一块。**只是选择**，按「就装这块盘」才真的写下去 ——
  // 写配置是有副作用的动作，不该在勾选的那一瞬间就发生
  const [picked, setPicked] = useState<string | null>(null)
  const target: DiskVolume | null = p
    ? picked
      ? (p.volumes.find((v) => v.mount === picked && !v.isSystem) ?? null)
      : chosen
    : null
  const targetMount = target?.mount ?? null
  const changed = target !== null && p !== null && target.mount !== p.chosen

  return (
    <WizardLayout
      title={t.disk.title}
      intro={t.disk.intro}
      footerLeft={
        <div className="flex items-center gap-[10px]">
          <Button size="sm" disabled={plan.loading} onClick={() => plan.reload()}>
            {t.common.recheck}
          </Button>
          {p && p.dataDir.trim() !== '' && (
            <Button size="sm" disabled={busy !== null} onClick={() => void choose('')}>
              {t.disk.useDefault}
            </Button>
          )}
        </div>
      }
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })} disabled={busy !== null}>
            {t.common.back}
          </Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            data-testid="disk-continue"
            disabled={targetMount === null || busy !== null || plan.loading}
            onClick={() => targetMount !== null && void choose(targetMount)}
          >
            {busy !== null ? t.common.working : t.disk.useThis}
          </Button>
        </>
      }
    >
      {plan.loading && <Spinner className="mt-[26px] text-amber" />}

      {!plan.loading && p === null && (
        <Card className="mt-[26px]">
          <div className="flex items-start gap-3">
            <AlertTriangle size={17} className="mt-[2px] text-danger" />
            <div className="min-w-0">
              <div className="text-xl font-medium leading-tight text-ink">{t.error.title}</div>
              <div className="mt-2 max-w-[660px] text-md leading-[1.5] text-body">
                {plan.error ? `${plan.error.code} · ${plan.error.message}` : t.app.noDataReason}
              </div>
            </div>
          </div>
        </Card>
      )}

      {p !== null && p.chosen === null && <NoDisk t={t} plan={p} />}

      {p !== null && p.chosen !== null && target !== null && (
        <>
          <Card className="mt-[26px]">
            <Row
              label={t.disk.whereLabel}
              value={
                <>
                  <span className="tnum">{`${target.mount}Hunter`}</span>
                  {target.hasHunter && <Tag>{t.disk.hunterTag}</Tag>}
                </>
              }
            />
            <Row label={t.disk.whyLabel} value={<Why t={t} plan={p} target={target} />} />
            <Row
              label={t.disk.sizeLabel}
              value={
                <>
                  {t.disk.firstInstall(p.firstInstallGb)}
                  <span className="text-muted">
                    {t.disk.partsNote}
                    {p.parts
                      .map((x) => t.disk.partLine(t.disk.part[x.key], approxGb(x.bytes)))
                      .join(t.disk.partsJoin)}
                    {t.disk.partsEnd}
                  </span>
                </>
              }
            />
            <Row
              label={t.disk.freeLabel}
              value={`${target.mount} ${t.disk.freeValue(freeGb(target))}`}
            />
          </Card>

          {/* **上限与实占分开写** —— 这一块单独立出来，就是不许和上面那行合 */}
          <Card className="mt-gap">
            <Row
              label={t.disk.capTitle}
              value={
                p.capGb === null ? (
                  <span className="text-muted">{t.disk.capUnknown}</span>
                ) : (
                  t.disk.capValue(p.capGb)
                )
              }
            />
            <Row label={t.disk.usedTitle} value={<Used t={t} plan={p} />} />
            <p className="mt-[10px] text-xs leading-[1.55] text-muted">{t.disk.capVsUsed}</p>
          </Card>

          <Card className="mt-gap">
            <div className="text-md font-medium text-ink">{t.disk.nextTitle}</div>
            <ul className="mt-[12px] flex flex-col gap-[8px]">
              {[t.disk.nextL2, t.disk.nextL3].map((x) => (
                <li key={x} className="flex gap-2.5 text-sm leading-[1.5] text-body">
                  <span className="mt-[7px] size-[5px] shrink-0 rounded-full bg-amber" />
                  <span className="min-w-0">{x}</span>
                </li>
              ))}
            </ul>
          </Card>

          {canChangeDisk(p) && (
            <Card className="mt-gap">
              <div className="text-md font-medium text-ink">{t.disk.change}</div>
              <div className="mt-[6px] text-xs leading-[1.55] text-muted">{t.disk.changeHint}</div>
              <div className="mt-[12px] flex flex-col gap-[8px]">
                {p.volumes
                  .filter((v) => !v.isSystem)
                  .map((v) => (
                    <DiskChoice
                      key={v.mount}
                      t={t}
                      v={v}
                      active={v.mount === target.mount}
                      onPick={() => setPicked(v.mount === p.chosen ? null : v.mount)}
                    />
                  ))}
              </div>
            </Card>
          )}

          {changed && (
            <p className="mt-[12px] text-sm leading-[1.55] text-amber-text">
              {t.disk.currentLabel} → {currentDir(p)}
            </p>
          )}
        </>
      )}

      {err && <div className="mt-[12px] text-sm text-danger">{err}</div>}
    </WizardLayout>
  )
}

type T = ReturnType<typeof useStore>['t']

/** 一块可选盘。选了不等于生效 —— 真的写下去要按底下那个主按钮。 */
function DiskChoice({
  t,
  v,
  active,
  onPick,
}: {
  t: T
  v: DiskVolume
  active: boolean
  onPick: () => void
}) {
  return (
    <button
      type="button"
      data-testid={`disk-option-${v.mount}`}
      onClick={onPick}
      className={`flex w-full items-center justify-between gap-4 rounded-lg border px-4 py-3 text-left transition-colors ${
        active ? 'border-amber bg-amber-soft' : 'border-line bg-card hover:border-line-strong'
      }`}
    >
      <span className="min-w-0">
        <span className="tnum text-md text-ink">{v.mount}</span>
        {v.hasHunter && <Tag>{t.disk.hunterTag}</Tag>}
      </span>
      <span className="shrink-0 text-sm text-body">
        {t.disk.sizeOf(v.mount, freeGb(v), Math.floor(v.totalBytes / 1024 ** 3))}
      </span>
    </button>
  )
}

/** 「为什么是它」—— 拿**真实数字**说清比较结果，不是只给一个分。 */
function Why({ t, plan, target }: { t: T; plan: DiskPlan; target: DiskVolume }) {
  const sys = systemVolume(plan)
  const best = largestNonSystem(plan)
  return (
    <span>
      {sys && <span className="text-muted">{t.disk.why.system(sys.mount, freeGb(sys))}</span>}
      {best && best.mount === target.mount && (
        <span>{t.disk.why.largest(target.mount, freeGb(target))}</span>
      )}
      {target.hasHunter && (
        <span className={best && best.mount === target.mount ? ' text-muted' : ''}>
          {t.disk.why.keeps(target.mount)}
        </span>
      )}
    </span>
  )
}

/** 「实际已占」那一行。三种情形三句话，一句都不许与上限合并。 */
function Used({ t, plan }: { t: T; plan: DiskPlan }) {
  const u = usedState(plan)
  if (u.kind === 'gb') return <span className="tnum">{t.disk.usedValue(u.gb)}</span>
  if (u.kind === 'none') return <span className="text-muted">{t.disk.usedNone}</span>
  return <span className="text-muted">{t.disk.usedUnknown}</span>
}

/**
 * `chosen === null`：S-01 说**一块合格的盘都没有**。
 *
 * 这不是异常，是正常返回值 —— 所以这里必须有话可说。两种情况分开说：
 * 「有盘但空间不够」（给出还差多少）与「连一块非系统盘都没有」。
 */
function NoDisk({ t, plan }: { t: T; plan: DiskPlan }) {
  const short = shortfall(plan)
  return (
    <Card className="mt-[26px]">
      <div className="flex items-start gap-3">
        <AlertTriangle size={17} className="mt-[2px] text-danger" />
        <div className="min-w-0">
          <div className="text-xl font-medium leading-tight text-ink">{t.disk.noneTitle}</div>
          <div className="mt-2 max-w-[660px] text-md leading-[1.5] text-body">
            {short
              ? t.disk.noneShort(short.mount, short.freeGb, short.shortGb)
              : t.disk.noneNoDisk(plan.minTotalGb)}
          </div>
        </div>
      </div>
    </Card>
  )
}

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-line py-[11px] last:border-b-0 last:pb-0">
      <div className="shrink-0 text-md text-body">{label}</div>
      <div className="min-w-0 text-right text-md leading-[1.5] text-ink">{value}</div>
    </div>
  )
}

function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span className="ml-2 rounded-sm border border-line px-[6px] py-[1px] text-xs text-muted">
      {children}
    </span>
  )
}
