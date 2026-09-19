import { Badge } from '../components/Badge'
import { Button, ChevronRight } from '../components/Button'
import { Card, CardHead } from '../components/Card'
import { EnvList } from '../components/EnvList'
import { LogBox } from '../components/LogBox'
import { ProgressBar } from '../components/ProgressBar'
import { ServiceTile } from '../components/ServiceTile'
import { StatCard } from '../components/StatCard'
import { StatusDot } from '../components/StatusDot'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { duration, percent, thousands } from '../lib/format'
import { useStore } from '../state/context'

/**
 * 运行面板 —— 视觉稿第 3 张。
 *
 * 与视觉稿的取值差异（都在 M0 里有实测依据）：
 *   · 端口是 3100 / 8100 / 3921 / 5442 / 6479，llm-shim 不发布端口显示「内部」（M0 §3.1、§5.3）；
 *   · 模型别名 hunter-chat，镜像源 GHCR；
 *   · 「今日对话」与「晨报」两张卡**显示「—」并给原因**：M0 §3.6 实测上游 api 里
 *     根本没有 /api/system/metrics/daily 这类接口，这两个数字没有任何数据来源。
 *     按总控规则红线 1，宁可空着也不编 —— 演示数据模式下同样是「—」。
 */
export function Dashboard() {
  const { t, locale, send, setOverlay, state } = useStore()
  const rt = useAsync(() => ipc.runtimeStatus(), [])
  const d = rt.data

  const services = d?.services ?? []
  const healthy = services.filter((s) => s.health === 'healthy').length
  const quota = d?.quota ?? null

  const running = state.name === 'Ready' && (d?.running ?? false)
  const hasUpdate = !!d?.latestTag && !!d.hunterTag && d.latestTag !== d.hunterTag

  return (
    <section className="flex min-h-0 flex-1 flex-col bg-window px-panel pb-panel pt-[40px]">
      {/* 头部：状态点 + 标题 + 打开按钮 */}
      <header className="flex shrink-0 items-start justify-between">
        <div className="flex items-center gap-[11px]">
          <StatusDot health={running ? 'healthy' : 'pending'} size={12} />
          <div>
            <h1 className="text-3xl font-semibold leading-none text-ink">
              {state.name === 'Stopped' ? t.dashboard.stopped : state.name === 'Upgrading' ? t.dashboard.starting : t.dashboard.running}
            </h1>
            <div className="tnum mt-[14px] text-md leading-none text-body">
              {d
                ? running
                  ? t.dashboard.subline(
                      `v${d.hunterTag ?? '—'}`,
                      duration(d.uptimeSeconds ?? 0, locale),
                      d.webUrl ?? '—',
                    )
                  : t.dashboard.sublineStopped(`v${d.hunterTag ?? '—'}`)
                : `${t.app.noData} · ${rt.error?.message ?? t.app.noDataReason}`}
            </div>
          </div>
        </div>
        <Button
          variant="primary"
          trailing={<ChevronRight />}
          disabled={!d?.webUrl}
          className="h-[46px] px-6 text-lg"
          onClick={() => d?.webUrl && void ipc.openExternal(d.webUrl)}
        >
          {t.dashboard.openHunter}
        </Button>
      </header>

      {/* 四张统计卡 */}
      <div className="mt-[30px] grid shrink-0 grid-cols-4 gap-gap">
        <StatCard
          label={t.dashboard.cardQuota}
          value={
            quota ? (
              <>
                {thousands(quota.usedToday)}
                <span className="text-md text-muted"> / {thousands(quota.limitDaily)}</span>
              </>
            ) : null
          }
          reason={t.app.noDataReason}
        >
          {quota && <ProgressBar value={percent(quota.usedToday, quota.limitDaily)} className="mt-[13px]" />}
        </StatCard>

        <StatCard
          label={t.dashboard.cardConversations}
          value={null}
          reason={t.dashboard.noSource}
        />
        <StatCard label={t.dashboard.cardBrief} value={null} reason={t.dashboard.noSource} />
        <StatCard
          label={t.dashboard.cardSource}
          value={d ? 'Hunter 网关' : null}
          mono={false}
          sub="DATA_SOURCE_PROVIDER=hunter"
          reason={t.app.noDataReason}
        />
      </div>

      {/* 下半区：左 服务+日志，右 环境+按钮 */}
      <div className="mt-gap grid min-h-0 flex-1 grid-cols-[3fr_2fr] gap-gap">
        <Card className="flex min-h-0 flex-col">
          <CardHead
            title={t.dashboard.services}
            right={
              <span className="tnum text-md text-muted">
                {services.length > 0 ? t.start.healthy(healthy, services.length) : t.app.noData}
              </span>
            }
          />
          <div className="mt-[14px] grid shrink-0 grid-cols-3 gap-[10px]">
            {services.map((s) => (
              <ServiceTile key={s.service} svc={s} internalLabel={t.common.internal} />
            ))}
          </div>
          <LogBox
            lines={d?.log ?? []}
            className="mt-[16px] min-h-0 flex-1"
            emptyText={rt.error?.message ?? t.logs.empty}
            highlight={/\d[\d,]*\s?tokens/}
          />
        </Card>

        <Card className="flex min-h-0 flex-col">
          <CardHead
            title={t.dashboard.env}
            right={hasUpdate ? <Badge tone="amber">{t.dashboard.updateBadge(`v${d!.latestTag}`)}</Badge> : undefined}
          />
          <div className="mt-[16px]">
            <EnvList
              items={[
                { label: t.dashboard.envModel, value: envOf(d, 'model'), reason: t.app.noDataReason },
                { label: t.dashboard.envDocker, value: envOf(d, 'docker'), reason: t.app.noDataReason },
                { label: t.dashboard.envRegistry, value: envOf(d, 'registry'), reason: t.app.noDataReason },
                { label: t.dashboard.envAutostart, value: boolLabel(envOf(d, 'autostart'), t.common.yes, t.common.no) },
                { label: t.dashboard.envTelemetry, value: boolLabel(envOf(d, 'telemetry'), t.common.yes, t.common.no) },
              ]}
            />
          </div>
          <div className="mt-auto flex flex-wrap gap-[10px] pt-4">
            <Button size="sm" onClick={() => send({ type: 'STOP' })}>
              {t.common.stop}
            </Button>
            <Button size="sm" onClick={() => send({ type: 'START' })}>
              {t.common.restart}
            </Button>
            <Button size="sm" onClick={() => setOverlay('logs')}>
              {t.common.logs}
            </Button>
            <Button size="sm" onClick={() => setOverlay('feedback')}>
              {t.common.feedback}
            </Button>
          </div>
        </Card>
      </div>
    </section>
  )
}

function envOf(d: { env: { key: string; value: string | null }[] } | null, key: string): string | null {
  return d?.env.find((e) => e.key === key)?.value ?? null
}

function boolLabel(v: string | null, on: string, off: string): string | null {
  if (v === null) return null
  return v === 'on' || v === 'true' ? on : off
}
