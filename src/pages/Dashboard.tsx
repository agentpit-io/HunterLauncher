import { useEffect, useState } from 'react'
import { Badge } from '../components/Badge'
import { Modal } from '../components/Modal'
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
import type { LauncherUpdate } from '../lib/types'
import { duration, percent, shanghaiStamp, thousands } from '../lib/format'
import { useStore } from '../state/context'

/**
 * 运行面板 —— 视觉稿第 3 张。
 *
 * 四张卡片的数据来源（M3 把上游能读的接口逐条探过一遍，结论写在成果文档里）：
 *
 *   | 卡片 | 来源 | 状态 |
 *   |---|---|---|
 *   | 今日模型额度 | 网关 GET /api/saas/llm/quota | 真数字 |
 *   | 数据源 | 本机 api GET /api/setup/status 的 llm.builtin / data_supply | 真数字（M3 接上） |
 *   | 今日对话 / 深度分析 | 没有接口 | 「—」+ 原因，清单见右下「需上游配合」 |
 *   | 晨报 | 没有接口 | 同上 |
 *
 * 与视觉稿的其它取值差异（都在 M0 里有实测依据）：
 *   · 端口是 3100 / 8100 / 3921 / 5442 / 6479，llm-shim 不发布端口显示「内部」（M0 §3.1、§5.3）；
 *   · 模型别名 hunter-chat，镜像源按真实测速结果显示。
 *
 * 按总控规则红线 1，拿不到的数字宁可空着也不编 —— 演示数据模式下同样是「—」。
 */
export function Dashboard() {
  const { t, locale, send, setOverlay, state } = useStore()
  const rt = useAsync(() => ipc.runtimeStatus(), [])
  const [busy, setBusy] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [showMissing, setShowMissing] = useState(false)
  // 后台每 24 小时查一次启动器自己的版本（方案 §10）。查到就在面板顶上挂一条
  const [launcherUpdate, setLauncherUpdate] = useState<LauncherUpdate | null>(null)
  const d = rt.data

  // 托盘也能启动 / 停止 / 重启容器，面板要跟着变。
  // 事件来的时候刷一次，另外每 10 秒兜底刷一次（容器自己挂掉不会有事件）。
  useEffect(() => {
    let un: (() => void) | undefined
    void ipc
      .onTrayAction((m) => {
        setNote(t.dashboard.trayNote(m))
        rt.reload()
      })
      .then((f) => {
        un = f
      })
    let un2: (() => void) | undefined
    void ipc.onLauncherUpdate(setLauncherUpdate).then((f) => {
      un2 = f
    })
    const timer = window.setInterval(() => rt.reload(), 10_000)
    return () => {
      un?.()
      un2?.()
      window.clearInterval(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /** 停止 / 重启都是真的去动容器，做完再刷新一次面板。 */
  async function act(action: 'stop' | 'restart') {
    setBusy(action)
    setNote(null)
    try {
      setNote(await ipc.stackAction(action))
      send(action === 'stop' ? { type: 'STOP' } : { type: 'START' })
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
      rt.reload()
    }
  }

  const services = d?.services ?? []
  const healthy = services.filter((s) => s.health === 'healthy').length
  const quota = d?.quota ?? null
  // 网关给的 resetAt 是 ISO（2026-09-21T00:00:00+08:00），上屏太长。
  // 网关自己写明了 reset_tz 是 Asia/Shanghai，所以只砍格式、**不做时区换算**
  //（和 Rust 侧 gateway::pretty_reset 同一套规则）。认不出来的格式返回 null，不猜。
  const quotaReset = shanghaiStamp(quota?.resetAt ?? null)

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
                : // 第一次拉数据要几秒（docker ps + 额度 + GitHub 三个往返），
                  // 这几秒里写「拿不到真实数据」是冤枉自己：还没拿完而已，不是拿不到。
                  rt.loading
                  ? t.common.loading
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

      {/* 启动器自己有新版本（方案 §10「有更新时托盘提示」，界面上也要有一条）。
          只在后台真的查到时才出现；点「查看」进更新页，装不装由用户决定 —— 绝不自动装。 */}
      {launcherUpdate?.available && launcherUpdate.version && (
        <div
          className="mt-[14px] flex shrink-0 items-center justify-between gap-4 rounded-md border border-amber/45 bg-amber-soft px-4 py-2.5"
          data-testid="launcher-update-banner"
        >
          <span className="tnum min-w-0 truncate text-sm text-amber-text">
            {t.update.launcherTitle} · {t.update.launcherLine(launcherUpdate.current, launcherUpdate.version)}
          </span>
          <div className="flex shrink-0 gap-[10px]">
            <Button size="sm" variant="ghost" onClick={() => setLauncherUpdate(null)}>
              {t.update.launcherLater}
            </Button>
            <Button size="sm" onClick={() => setOverlay('update')}>
              {t.update.launcherNow}
            </Button>
          </div>
        </div>
      )}

      {/* api 自报 key 有没有落进容器（M0 §3.1 的意外收获）。只有明确是 false 才提示，
          读不到就什么都不说 —— 没读到不等于没配上（红线 1）。 */}
      {d?.apiKeyConfigured === false && (
        <div className="mt-[14px] shrink-0 rounded-md border border-danger/40 bg-card px-4 py-2.5 text-sm text-danger">
          {t.dashboard.keyMissing}
        </div>
      )}

      {/* 今日额度用完了（I3 回归实测撞出来的）。
          输入 key 那一页白纸黑字写着「额度用尽会明确提示，不会静默降级」，
          可是面板上原来只摆着 304,144 / 300,000 两个数字 —— 用户在 Hunter 里
          对话被挡住，回到这里看不出任何原因。这条横幅就是那句承诺的兑现。
          只有网关明确说 exhausted（或 remaining ≤ 0）时才出现，读不到额度什么都不说。 */}
      {quota?.exhausted && (
        <div
          className="mt-[14px] shrink-0 rounded-md border border-danger/40 bg-danger-soft px-4 py-2.5"
          data-testid="quota-exhausted-banner"
        >
          <div className="text-sm font-medium text-danger">{t.dashboard.quotaExhaustedTitle}</div>
          <div className="tnum mt-[4px] text-sm leading-[1.45] text-body">
            {t.dashboard.quotaExhaustedBody(
              thousands(quota.usedToday),
              thousands(quota.limitDaily),
              quotaReset,
            )}
          </div>
        </div>
      )}

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
          sub={
            quota?.exhausted ? (
              <span className="text-danger">{t.dashboard.quotaExhaustedCard}</span>
            ) : undefined
          }
          reason={t.app.noDataReason}
        >
          {quota && (
            <ProgressBar
              value={percent(quota.usedToday, quota.limitDaily)}
              tone={quota.exhausted ? 'danger' : 'amber'}
              className="mt-[13px]"
            />
          )}
        </StatCard>

        {/* 今日对话 / 晨报：上游没有接口，显示「—」并把原因写清楚（红线 1）。
            点右下角的「需上游配合」能看到缺的是哪几条接口。 */}
        <StatCard
          label={t.dashboard.cardConversations}
          value={null}
          reason={t.dashboard.noSource}
        />
        <StatCard label={t.dashboard.cardBrief} value={null} reason={t.dashboard.noSource} />
        {/* 数据源：真读本机 api 的 /api/setup/status。
            副行不写「自有数据源 0 个」—— 那个计数要登录态才读得到，写 0 就是编（红线 1）。 */}
        <StatCard
          label={t.dashboard.cardSource}
          value={d?.dataSource ?? null}
          mono={false}
          sub={d?.dataSourceSub}
          reason={d?.dataSourceSub ?? t.app.noDataReason}
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
            right={
              hasUpdate ? (
                // 视觉稿第 3 张右上角那个「有新版本 v1.0.0」的角标。M4 起它是可点的：
                // 点开就是更新页（Release Notes 摘要 + 升级 + 备份）
                <button
                  type="button"
                  data-testid="hunter-update-badge"
                  onClick={() => setOverlay('update')}
                  className="transition-opacity hover:opacity-80"
                >
                  <Badge tone="amber">{t.dashboard.updateBadge(`v${d!.latestTag}`)}</Badge>
                </button>
              ) : undefined
            }
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
          {(d?.missing?.length ?? 0) > 0 && (
            <button
              type="button"
              data-testid="missing-endpoints"
              onClick={() => setShowMissing(true)}
              className="mt-[12px] self-start text-xs text-muted underline decoration-dotted underline-offset-4 transition-colors hover:text-amber-text"
            >
              {t.dashboard.missingTitle}（{d!.missing.length}）
            </button>
          )}
          {note && <div className="mt-[12px] text-sm leading-[1.5] text-amber-text">{note}</div>}
          <div className="mt-auto flex flex-wrap gap-[10px] pt-4">
            <Button size="sm" disabled={busy !== null} onClick={() => void act('stop')}>
              {busy === 'stop' ? t.common.working : t.common.stop}
            </Button>
            <Button size="sm" disabled={busy !== null} onClick={() => void act('restart')}>
              {busy === 'restart' ? t.common.working : t.common.restart}
            </Button>
            <Button size="sm" onClick={() => setOverlay('logs')}>
              {t.common.logs}
            </Button>
            <Button size="sm" data-testid="open-update" onClick={() => setOverlay('update')}>
              {t.update.hunterCheck}
            </Button>
            <Button size="sm" onClick={() => setOverlay('feedback')}>
              {t.common.feedback}
            </Button>
          </div>
        </Card>
      </div>

      {showMissing && d && (
        <Modal
          testId="missing-dialog"
          title={t.dashboard.missingTitle}
          onClose={() => setShowMissing(false)}
          footer={
            <Button size="sm" onClick={() => setShowMissing(false)}>
              {t.common.close}
            </Button>
          }
        >
          <p className="text-sm leading-[1.6] text-body">{t.dashboard.missingHint}</p>
          <ul className="tnum mt-[14px] flex flex-col gap-[8px] text-sm text-dim">
            {d.missing.map((m) => (
              <li key={m.id} className="break-all">
                · {m.endpoint}
              </li>
            ))}
          </ul>
        </Modal>
      )}
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
