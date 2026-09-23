import { Badge } from './Badge'
import { Card, CardHead } from './Card'
import { usePoll } from '../lib/usePoll'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { useStore } from '../state/context'
import type { MonitorLevel, ServiceUsage } from '../lib/types'

/**
 * 运行面板上半部的三层资源（I12 · R2 · 方案 R2 的草图）。
 *
 * ```
 * ┌ 这台电脑 ─────────┐┌ Hunter 运行环境 ───┐┌ 数据 ─────────────────┐
 * │ CPU   18%         ││ 4 核 · 4 GB        ││ 数据库  312 MB          │
 * │ 内存  11.2/16 GB  ││ 内存已用 2.1 GB    ││ 数据卷合计 380 MB       │
 * │       压力：正常   ││ 磁盘 7.9/60 GB     ││ 镜像 4.0 GB             │
 * │ 系统盘 剩 223 GB  ││                    ││                         │
 * └───────────────────┘└────────────────────┘└─────────────────────────┘
 * 服务  web ● 1.2% 135MB · api ● 0.5% 121MB · …
 * ```
 *
 * ## 三层分开的理由（也是每张卡上那行小字的理由）
 *
 * macOS 上 Hunter 跑在一台虚拟机里。「这台 Mac 内存 92%」和「虚拟机内存 92%」
 * 是两件完全不同的事，处理办法也不同（关别的程序 / 给虚拟机多分一点）。
 * 所以每张卡的标题旁边都写着**这一层的数字是从哪问来的**。
 *
 * ## 刷新频率不一样，所以是四条独立的轮询
 *
 * 这台电脑 5 秒、运行环境 30 秒、各服务 10 秒、数据卷与镜像 5 分钟（方案 R2 的表）。
 * 合成一条的话，最慢的那一层（`docker system df -v` 要遍历所有卷）会把最快的拖住。
 * 四条都在窗口隐藏时停（见 [`usePoll`]）。
 *
 * ## 拿不到就显示「—」
 *
 * 每个后端结构里都带一份 `reasons`，逐项写着「这一项为什么没有」。
 * 这里把它挂在 `title` 上，鼠标停上去就能看到原话 —— 绝不拿 0 或者上一次的值顶替（红线 1）。
 */
export function ResourcePanel() {
  const { t } = useStore()
  const host = usePoll(() => ipc.monitorHost(), 5_000)
  const runtime = usePoll(() => ipc.monitorRuntime(), 30_000)
  const services = usePoll(() => ipc.monitorServices(), 10_000)
  const storage = usePoll(() => ipc.monitorStorage(), 300_000)

  const h = host.data
  const r = runtime.data
  const s = storage.data

  return (
    <div className="mt-[18px] flex shrink-0 flex-col gap-[10px]" data-testid="resource-panel">
      <div className="grid grid-cols-3 gap-gap">
        {/* ── 这台电脑 ── */}
        <Card padded={false} className="flex min-w-0 flex-col p-[16px]">
          <CardHead
            title={t.monitor.host}
            right={<Badge tone="neutral">{t.monitor.hostSource}</Badge>}
          />
          <div className="mt-[12px] flex flex-col gap-[7px]">
            <Row
              label={t.monitor.cpu}
              value={h?.cpuPct === null || h?.cpuPct === undefined ? null : `${h.cpuPct.toFixed(0)}%`}
              sub={h?.cpuCores ? t.monitor.cores(h.cpuCores) : undefined}
              reason={reasonOf(host.error, h?.reasons, 'cpu', t.app.noDataReason)}
            />
            <Row
              label={t.monitor.mem}
              value={pair(h?.memUsedBytes, h?.memTotalBytes)}
              reason={reasonOf(host.error, h?.reasons, 'mem', t.app.noDataReason)}
            />
            <Row
              label={t.monitor.pressure}
              value={h?.memPressure ?? null}
              level={h?.memPressureLevel}
              reason={reasonOf(host.error, h?.reasons, 'memPressure', t.app.noDataReason)}
            />
            <Row
              label={t.monitor.systemDisk}
              value={h?.diskFreeBytes === null || h?.diskFreeBytes === undefined
                ? null
                : t.monitor.freeOf(bytes(h.diskFreeBytes), h.diskTotalBytes ? bytes(h.diskTotalBytes) : '—')}
              level={h?.diskLevel}
              reason={reasonOf(host.error, h?.reasons, 'disk', t.app.noDataReason)}
            />
          </div>
        </Card>

        {/* ── Hunter 运行环境（只有内置运行时才有这一层）── */}
        <Card padded={false} className="flex min-w-0 flex-col p-[16px]">
          <CardHead
            title={t.monitor.runtime}
            right={<Badge tone="neutral">{t.monitor.runtimeSource}</Badge>}
          />
          {r && !r.applicable ? (
            // 用户自己的 OrbStack / Docker Desktop / 系统 docker：这一层根本不存在。
            // **不给一组 0** —— 那是编数字
            <div className="mt-[12px] text-sm leading-[1.5] text-muted" data-testid="runtime-na">
              {t.monitor.runtimeNa}
              <div className="mt-[6px] text-xs leading-[1.5] text-dim">{r.reason}</div>
            </div>
          ) : (
            <div className="mt-[12px] flex flex-col gap-[7px]">
              <Row
                label={t.monitor.vmSpec}
                value={
                  r?.cpus
                    ? t.monitor.vmSpecValue(r.cpus, r.memTotalBytes ? bytes(r.memTotalBytes) : '—')
                    : null
                }
                reason={runtime.error ?? t.app.noDataReason}
              />
              <Row
                label={t.monitor.vmMem}
                value={pair(r?.memUsedBytes, r?.memTotalBytes)}
                level={r?.memLevel}
                reason={reasonOf(runtime.error, r?.reasons, 'mem', t.app.noDataReason)}
              />
              <Row
                label={t.monitor.vmDisk}
                value={pair(r?.diskUsedBytes, r?.diskTotalBytes)}
                level={r?.diskLevel}
                reason={reasonOf(runtime.error, r?.reasons, 'disk', t.app.noDataReason)}
              />
            </div>
          )}
        </Card>

        {/* ── 数据 ── */}
        <Card padded={false} className="flex min-w-0 flex-col p-[16px]">
          <CardHead
            title={t.monitor.data}
            right={<Badge tone="neutral">{t.monitor.dataSource}</Badge>}
          />
          <div className="mt-[12px] flex flex-col gap-[7px]">
            <Row
              label={t.monitor.db}
              value={s?.dbBytes === null || s?.dbBytes === undefined ? null : bytes(s.dbBytes)}
              reason={reasonOf(storage.error, s?.reasons, 'volumeSizes', t.app.noDataReason)}
            />
            <Row
              label={t.monitor.volumes}
              value={
                s?.volumesTotalBytes === null || s?.volumesTotalBytes === undefined
                  ? null
                  : bytes(s.volumesTotalBytes)
              }
              sub={s ? t.monitor.volumeCount(s.volumes.length) : undefined}
              reason={reasonOf(storage.error, s?.reasons, 'volumes', t.app.noDataReason)}
            />
            <Row
              label={t.monitor.images}
              value={s?.imagesBytes === null || s?.imagesBytes === undefined ? null : bytes(s.imagesBytes)}
              // 六个里查到几个要说清楚 —— 少查到一个，这个合计就是偏小的
              sub={s && s.imagesFound > 0 ? t.monitor.imagesFound(s.imagesFound) : undefined}
              reason={reasonOf(storage.error, s?.reasons, 'images', t.app.noDataReason)}
            />
          </div>
        </Card>
      </div>

      {/* ── 一行服务用量 ── */}
      <Card padded={false} className="px-[16px] py-[11px]">
        <div className="flex min-w-0 items-center gap-[10px] overflow-x-auto">
          <span className="shrink-0 text-sm text-muted">{t.monitor.services}</span>
          {services.data && services.data.services.length > 0 ? (
            services.data.services.map((u) => <Usage key={u.service} u={u} />)
          ) : (
            <span className="text-sm text-muted">
              {services.data?.reason || services.error || t.common.loading}
            </span>
          )}
        </div>
      </Card>
    </div>
  )
}

function Usage({ u }: { u: ServiceUsage }) {
  const { t } = useStore()
  const dead = u.oomKilled === true
  const restarted = (u.restartCount ?? 0) > 0
  return (
    <span
      className="tnum flex shrink-0 items-center gap-[6px] text-sm text-body"
      data-testid={`usage-${u.service}`}
      title={
        u.restartCount === null
          ? undefined
          : t.monitor.restartTip(u.restartCount, u.oomKilled === true)
      }
    >
      <span
        className={`size-[6px] rounded-full ${dead ? 'bg-danger' : restarted ? 'bg-amber' : 'bg-success'}`}
      />
      {u.service}
      <span className="text-muted">
        {u.cpuPct === null ? '—' : `${u.cpuPct.toFixed(1)}%`}{' '}
        {u.memBytes === null ? '—' : bytes(u.memBytes, 0)}
      </span>
    </span>
  )
}

/** 一行「标签 ······ 值」。值为 null 时显示「—」，并把原因挂在 title 上（红线 1）。 */
function Row({
  label,
  value,
  sub,
  level,
  reason,
}: {
  label: string
  value: string | null | undefined
  sub?: string
  level?: MonitorLevel
  reason: string
}) {
  const empty = value === null || value === undefined
  const tone =
    level === 'crit' ? 'text-danger' : level === 'warn' ? 'text-amber-text' : 'text-ink'
  return (
    <div className="flex min-w-0 items-baseline justify-between gap-3">
      <span className="shrink-0 text-sm leading-[1.3] text-muted">{label}</span>
      <span
        className={`tnum min-w-0 truncate text-right text-sm leading-[1.3] ${empty ? 'text-muted' : tone}`}
        title={empty ? reason : undefined}
      >
        {empty ? '—' : value}
        {!empty && sub && <span className="ml-1.5 text-muted">{sub}</span>}
      </span>
    </div>
  )
}

function pair(used: number | null | undefined, total: number | null | undefined): string | null {
  if (used === null || used === undefined) return null
  return total ? `${bytes(used)} / ${bytes(total)}` : bytes(used)
}

/**
 * 这一项为什么没有。
 *
 * 优先用后端逐项给的原因（`reasons[key]`）；整轮没采到就用这一轮的错误；
 * 两个都没有才落到那句最泛的「拿不到真实数据」。**任何一档都不是空白。**
 */
function reasonOf(
  pollError: string | null,
  reasons: Record<string, string> | undefined,
  key: string,
  fallback: string,
): string {
  return reasons?.[key] ?? pollError ?? fallback
}
