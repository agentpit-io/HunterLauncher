import { useEffect, useState } from 'react'
import { AlertBanner } from '../components/AlertBanner'
import { InterruptedUpgradeCard } from '../components/InterruptedUpgradeCard'
import { UploadLogs } from '../components/UploadLogs'
import { Badge } from '../components/Badge'
import { Modal } from '../components/Modal'
import { UninstallDialog } from '../components/UninstallDialog'
import { Button, ChevronRight } from '../components/Button'
import { Card, CardHead } from '../components/Card'
import { EnvList } from '../components/EnvList'
import { LogBox } from '../components/LogBox'
import { ProgressBar } from '../components/ProgressBar'
import { ResourcePanel } from '../components/ResourcePanel'
import { ServiceTile } from '../components/ServiceTile'
import { StatCard } from '../components/StatCard'
import { StatusDot } from '../components/StatusDot'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import type { BackupSettings, LauncherUpdate, StackOpResult, StackPlan } from '../lib/types'
import { bytes, duration, percent, shanghaiStamp, thousands } from '../lib/format'
import { backupOverdue } from '../lib/danger'
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
  const { t, locale, send, setOverlay, state, notice, setNotice } = useStore()
  const rt = useAsync(() => ipc.runtimeStatus(), [])
  const [busy, setBusy] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [showMissing, setShowMissing] = useState(false)
  // 后台每 24 小时查一次启动器自己的版本（方案 §10）。查到就在面板顶上挂一条
  const [launcherUpdate, setLauncherUpdate] = useState<LauncherUpdate | null>(null)
  // I7：一键把网页端口收回本机。只有升级上来的老机器会用到
  const [tightening, setTightening] = useState(false)
  // I12 · R3：停止 / 启动 / 重启
  const [dialog, setDialog] = useState<'stop' | 'restart' | null>(null)
  const [alsoRuntime, setAlsoRuntime] = useState(true)
  const [pickedService, setPickedService] = useState('')
  const [opSteps, setOpSteps] = useState<string[]>([])
  const [opResult, setOpResult] = useState<StackOpResult | null>(null)
  const [plan, setPlan] = useState<StackPlan | null>(null)
  // I13 · R4 / R6：删除应用放在「更多 ▾」里（方案第三节第 2 条：颜色弱化，不放主按钮区）
  const [more, setMore] = useState(false)
  // 截图脚本用 HUNTER_DEMO_PAGE=uninstall / uninstall-all 直接把这个弹窗打开
  const [uninstalling, setUninstalling] = useState(
    () => ipc.demoPage() === 'uninstall' || ipc.demoPage() === 'uninstall-all',
  )
  const [backupNonce, setBackupNonce] = useState(0)
  const [schedRetrying, setSchedRetrying] = useState(false)
  const bk = useAsync<BackupSettings>(() => ipc.readBackupSettings(), [backupNonce])
  const [backingUp, setBackingUp] = useState(false)
  const d = rt.data

  async function onTighten() {
    setTightening(true)
    try {
      setNote(await ipc.tightenWebBind())
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setTightening(false)
      rt.reload()
    }
  }

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
    // I12 · R3：操作过程中后端每做完一件事推一条，直播给用户看
    let un3: (() => void) | undefined
    void ipc.onStackStep((line) => setOpSteps((v) => [...v, line])).then((f) => {
      un3 = f
    })
    const timer = window.setInterval(() => rt.reload(), 10_000)
    return () => {
      un?.()
      un2?.()
      un3?.()
      window.clearInterval(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /**
   * 停止 / 启动 / 重启（I12 · R3）。
   *
   * 和 0.1.11 的 `stackAction` 比多了三件事：停止可以顺带停掉运行环境、
   * 启动会先把运行环境弄好、重启可以只重启一个服务。
   * 过程里后端每做完一件事就推一条 `hunter://stack`，这里直播出来 ——
   * 按钮置灰的那几十秒不该是一片空白。
   */
  async function act(action: 'stop' | 'start' | 'restart', opts: { alsoRuntime?: boolean; service?: string } = {}) {
    setBusy(action)
    setNote(null)
    setOpSteps([])
    setOpResult(null)
    setDialog(null)
    try {
      const r = await ipc.stackOp(action, opts)
      setOpResult(r)
      setNote(r.headline)
      send(action === 'stop' ? { type: 'STOP' } : { type: 'START' })
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
      rt.reload()
    }
  }

  /** 面板上那条「立即备份一次」。做完刷新那一行，失败就把原话摆出来。 */
  // I16 · P0-3：红横幅上的「再挂一次」。成了横幅自己消失，没成就换成新的原话
  async function retrySchedule() {
    setSchedRetrying(true)
    try {
      await ipc.retryBackupSchedule()
    } finally {
      setSchedRetrying(false)
      setBackupNonce((v) => v + 1)
    }
  }

  async function backupNow() {
    setBackingUp(true)
    setNote(null)
    try {
      const m = await ipc.createBackup()
      setNote(`${m.id} · ${bytes(m.totalBytes)}`)
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e))
    } finally {
      setBackingUp(false)
      setBackupNonce((n) => n + 1)
    }
  }

  /** 打开「停止」对话框之前先问后端：这台机器要不要显示「顺便停运行环境」。 */
  async function openStop() {
    setDialog('stop')
    try {
      setPlan(await ipc.stackPlan())
    } catch {
      // 问不到就当没有那一档 —— 界面上不出现一个不知道会做什么的勾选
      setPlan(null)
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
            {/* I16 · P0-4：**大标题跟着「现在到底有没有在跑」走，不跟状态机的名字走。**
                0.1.15 只在 `state.name === 'Stopped'` 时才写「已停止」，
                于是客户那台 Windows 上（状态机停在 Ready、六个容器全 exited）
                出现了「Hunter 运行中」＋下一行「v1.2.2 · 容器已停止」这种自相矛盾的一屏。
                还没拿到数据的那几秒说「正在检查」—— 那时候说「已停止」是冤枉自己。 */}
            <h1 className="text-3xl font-semibold leading-none text-ink" data-testid="dash-title">
              {state.name === 'Upgrading'
                ? t.dashboard.starting
                : !d
                  ? t.dashboard.checking
                  : running
                    ? t.dashboard.running
                    : t.dashboard.stopped}
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

      {/* I11 · U1：错误页复查发现「其实已经好了」，自己把界面切到了这里 ——
          切过来之后必须有人说一句刚才发生了什么，否则那一下会像界面自己乱跳。
          这句话来自后端的复查结论（真实的服务数与 HTTP 状态码），不是前端编的。 */}
      {notice && (
        <div
          className="mt-[14px] shrink-0 rounded-md border border-success/40 bg-success/5 px-4 py-2.5"
          data-testid="recovered-banner"
        >
          <div className="flex items-center justify-between gap-[14px]">
            <div className="min-w-0">
              <div className="text-sm font-medium text-ok">{t.dashboard.recoveredTitle}</div>
              <div className="mt-[4px] text-sm leading-[1.45] text-body">{notice}</div>
              <div className="mt-[4px] text-xs leading-[1.45] text-muted">
                {t.dashboard.recoveredBody}
              </div>
            </div>
            <Button size="sm" variant="ghost" onClick={() => setNotice(null)}>
              {t.common.gotIt}
            </Button>
          </div>
        </div>
      )}

      {/* I7：这台机器的网页端口现在对局域网开着（只会出现在升级上来的老机器上）。
          用户 2026-09-21 19:05 的决定第三点：升级**不自动改动**已有配置，
          但要在这里说一句现状，并给一个「只允许本机访问」。收紧是单向的。
          依据是 docker 报的真实绑定地址，不是配置（红线 1）。 */}
      {d?.webLanExposed && (
        <div
          className="mt-[14px] shrink-0 rounded-md border border-amber/40 bg-amber/5 px-4 py-2.5"
          data-testid="web-lan-banner"
        >
          <div className="flex items-center justify-between gap-[14px]">
            <div className="min-w-0">
              <div className="text-sm font-medium text-amber-text">{t.dashboard.lanTitle}</div>
              <div className="mt-[4px] text-sm leading-[1.45] text-body">{t.dashboard.lanBody}</div>
              <div className="mt-[4px] text-xs leading-[1.45] text-muted">
                {t.dashboard.lanOneWay}
              </div>
            </div>
            <Button
              size="sm"
              data-testid="web-lan-tighten"
              disabled={tightening}
              onClick={() => void onTighten()}
            >
              {tightening ? t.dashboard.lanTighteningNote : t.dashboard.lanTighten}
            </Button>
          </div>
        </div>
      )}

      {/* I16 · P1-2：上一次升级没做完（强退留下的中间态）。
          **排在所有提醒的最前面** —— 这台机器现在的「版本」这件事本身是说不清的，
          在用户选定之前，面板上其它关于版本的数字都要打个问号 */}
      <InterruptedUpgradeCard onResolved={() => rt.reload()} />

      {/* I13 · R7：硬盘 / 内存 / 服务异常的提醒。排在资源卡之前 ——
          有问题的时候，用户要先看到结论，再去看那一堆数字 */}
      <AlertBanner />

      {/* I16 · P0-3：**定时任务根本没挂上**。排在「上一次备份失败」之前 ——
          「某一次没成」和「一次都不会跑」是两件事，后者严重得多。
          客户那台 Windows（0.1.15）从装机第一天起就是这样，
          日志里两行 `schtasks /Create 失败`，界面上一个字都没有。 */}
      {bk.data && bk.data.enabled && bk.data.scheduleError && (
        <div
          className="mt-[14px] flex shrink-0 items-center justify-between gap-4 rounded-md border border-danger/45 bg-danger-soft px-4 py-2.5"
          data-testid="schedule-banner"
        >
          <span className="min-w-0 text-sm leading-[1.45] text-danger">
            {t.dashboard.scheduleBroken(bk.data.scheduleError)}
          </span>
          <div className="flex shrink-0 gap-[10px]">
            <Button
              size="sm"
              data-testid="schedule-retry"
              disabled={schedRetrying}
              onClick={() => void retrySchedule()}
            >
              {schedRetrying ? t.dashboard.scheduleRetrying : t.dashboard.scheduleRetry}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setOverlay('backup')}>
              {t.dashboard.openBackup}
            </Button>
          </div>
        </div>
      )}

      {/* I13 · R6：上一次自动备份失败了，或者它好像错过了一次。
          这条只在**真的出事**时出现；一切正常时面板上一个字都不多 */}
      {bk.data && (bk.data.lastError || backupOverdue(bk.data)) && (
        <div
          className={`mt-[14px] flex shrink-0 items-center justify-between gap-4 rounded-md border px-4 py-2.5 ${
            bk.data.lastError ? 'border-danger/45 bg-danger-soft' : 'border-amber/45 bg-amber-soft'
          }`}
          data-testid="backup-banner"
        >
          <span className={`min-w-0 text-sm leading-[1.45] ${bk.data.lastError ? 'text-danger' : 'text-amber-text'}`}>
            {bk.data.lastError ? t.dashboard.backupFailed(bk.data.lastError) : t.dashboard.missedBackup}
          </span>
          <div className="flex shrink-0 gap-[10px]">
            <Button
              size="sm"
              data-testid="backup-now"
              disabled={backingUp}
              onClick={() => void backupNow()}
            >
              {backingUp ? t.dashboard.backingUp : t.dashboard.backupNow}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setOverlay('backup')}>
              {t.dashboard.openBackup}
            </Button>
          </div>
        </div>
      )}

      {/* I12 · R2：三层资源。放在统计卡之前 —— 用户打开面板最先想知道的是
          「它现在占了我多少东西」，而不是额度还剩多少 */}
      <ResourcePanel />

      {/* 四张统计卡 */}
      <div className="mt-[18px] grid shrink-0 grid-cols-4 gap-gap">
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
                // I12 · R1：启动器记住的那条安装记录。摆出来才看得见它对不对
                { label: t.dashboard.envInstalledAt, value: envOf(d, 'installedAt'), reason: t.dashboard.envInstalledAtNone },
                { label: t.dashboard.envLastHealthy, value: envOf(d, 'lastHealthy'), reason: t.dashboard.envLastHealthyNone },
                // I13 · R6：最近一次备份。失败过就标红 —— 这一行是「备份到底有没有在跑」唯一看得见的地方
                {
                  label: t.dashboard.envLastBackup,
                  value: bk.data?.lastError
                    ? null
                    : bk.data?.lastOkAt
                      ? `${bk.data.lastOkAt}（${bk.data.count} 份 · ${bytes(bk.data.totalBytes)}）`
                      : null,
                  reason: bk.data?.lastError
                    ? t.dashboard.backupFailed(bk.data.lastError)
                    : t.dashboard.envLastBackupNone,
                },
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
          {/* I12 · R3：操作进行中把刚做过的事直播出来 —— 按钮置灰的那几十秒
              不该是一片空白（后端每做完一件事推一条 hunter://stack） */}
          {(opSteps.length > 0 || opResult) && (
            <div className="mt-[12px] rounded-md border border-line bg-window px-3 py-2" data-testid="stack-steps">
              <div className="text-xs text-muted">{t.stackOp.steps}</div>
              <ul className="mt-[6px] flex flex-col gap-[4px]">
                {(opResult?.steps ?? opSteps).map((l, i) => (
                  <li key={`${i}-${l}`} className="text-xs leading-[1.45] text-body">
                    · {l}
                  </li>
                ))}
              </ul>
              {opResult && (
                <div className="tnum mt-[6px] text-xs text-muted">
                  {t.stackOp.doneIn(opResult.elapsedMs)} · {opResult.ready} / {opResult.total}
                </div>
              )}
            </div>
          )}
          <div className="mt-auto flex flex-wrap gap-[10px] pt-4">
            {/* 已经停了的时候主按钮是「启动」——「停止」在那儿没有意义 */}
            {running ? (
              <Button size="sm" data-testid="stack-stop" disabled={busy !== null} onClick={() => void openStop()}>
                {busy === 'stop' ? t.common.working : t.common.stop}
              </Button>
            ) : (
              <Button
                size="sm"
                variant="primary"
                data-testid="stack-start"
                disabled={busy !== null}
                onClick={() => void act('start')}
              >
                {busy === 'start' ? t.common.working : t.common.start}
              </Button>
            )}
            <Button
              size="sm"
              data-testid="stack-restart"
              disabled={busy !== null}
              onClick={() => {
                setPickedService('')
                setDialog('restart')
              }}
            >
              {busy === 'restart' ? t.common.working : t.common.restart}
            </Button>
            <Button size="sm" onClick={() => setOverlay('logs')}>
              {t.common.logs}
            </Button>
            <Button size="sm" data-testid="open-update" onClick={() => setOverlay('update')}>
              {t.update.hunterCheck}
            </Button>
            <Button size="sm" data-testid="open-backup" onClick={() => setOverlay('backup')}>
              {t.dashboard.openBackup}
            </Button>
            <Button size="sm" onClick={() => setOverlay('feedback')}>
              {t.common.feedback}
            </Button>
            {/* I16：诊断区的第三个出口 —— 点一下，脱敏后的日志直接进我们的排障库，
                用户只要念一个追踪码。上面那两个（日志 / 反馈）都要他自己动手发文件 */}
            <UploadLogs stage="dashboard" testId="dashboard-upload-logs" />
            {/* 「更多 ▾」：删除应用放在这里，不放主按钮区（方案第三节第 2 条） */}
            <div className="relative">
              <Button size="sm" variant="ghost" data-testid="open-more" onClick={() => setMore((v) => !v)}>
                {t.dashboard.more} ▾
              </Button>
              {more && (
                <div
                  className="absolute bottom-[40px] right-0 z-20 w-[160px] rounded-md border border-line-strong bg-card py-1 shadow-2xl"
                  data-testid="more-menu"
                >
                  <button
                    type="button"
                    data-testid="open-uninstall"
                    onClick={() => {
                      setMore(false)
                      setUninstalling(true)
                    }}
                    className="w-full px-3 py-2 text-left text-sm text-danger transition-colors hover:bg-danger-soft"
                  >
                    {t.dashboard.uninstall}
                  </button>
                </div>
              )}
            </div>
          </div>
        </Card>
      </div>

      {/* I12 · R3：停止前问一句「顺便停运行环境吗」。
          只有内置运行时、而且它现在在跑的机器才有这一问（stack_plan 说了算）。
          默认勾上 —— 用户点「停止」想要的就是「把它占的资源放掉」 */}
      {dialog === 'stop' && (
        <Modal
          testId="stop-dialog"
          title={t.stackOp.stopTitle}
          onClose={() => setDialog(null)}
          footer={
            <>
              <Button size="sm" variant="ghost" onClick={() => setDialog(null)}>
                {t.common.cancel}
              </Button>
              <Button
                size="sm"
                variant="primary"
                data-testid="stop-confirm"
                onClick={() => void act('stop', { alsoRuntime: (plan?.builtinRunning ?? false) && alsoRuntime })}
              >
                {t.stackOp.stopConfirm}
              </Button>
            </>
          }
        >
          <p className="text-sm leading-[1.6] text-body">{t.stackOp.stopBody}</p>
          {plan?.builtinRunning && (
            <label className="mt-[14px] flex cursor-pointer items-start gap-2.5" data-testid="stop-also-runtime">
              <input
                type="checkbox"
                className="mt-[3px] size-[14px] accent-amber"
                checked={alsoRuntime}
                onChange={(e) => setAlsoRuntime(e.target.checked)}
              />
              <span>
                <span className="text-sm text-ink">{t.stackOp.stopAlsoRuntime(plan.builtinMemGb)}</span>
                <span className="mt-[4px] block text-xs leading-[1.5] text-muted">
                  {t.stackOp.stopAlsoRuntimeHint}
                </span>
              </span>
            </label>
          )}
        </Modal>
      )}

      {/* I12 · R3：重启。可以只重启一个服务 —— 一个服务卡住时，
          把六个一起推倒重来既慢又可能把别的弄坏 */}
      {dialog === 'restart' && (
        <Modal
          testId="restart-dialog"
          title={t.common.restart}
          onClose={() => setDialog(null)}
          footer={
            <>
              <Button size="sm" variant="ghost" onClick={() => setDialog(null)}>
                {t.common.cancel}
              </Button>
              <Button size="sm" data-testid="restart-all" onClick={() => void act('restart')}>
                {t.stackOp.restartAll}
              </Button>
              <Button
                size="sm"
                variant="primary"
                data-testid="restart-one"
                disabled={!pickedService}
                onClick={() => void act('restart', { service: pickedService })}
              >
                {t.stackOp.restartOne}
              </Button>
            </>
          }
        >
          <p className="text-sm leading-[1.6] text-body">{t.stackOp.pickService}</p>
          <div className="mt-[14px] flex flex-wrap gap-[8px]">
            {services.map((sv) => (
              <button
                key={sv.service}
                type="button"
                data-testid={`pick-${sv.service}`}
                onClick={() => setPickedService(sv.service)}
                className={`rounded-sm border px-3 py-1.5 text-sm transition-colors ${
                  pickedService === sv.service
                    ? 'border-amber/60 bg-amber-soft text-amber-text'
                    : 'border-line-strong bg-card text-body hover:border-amber/40'
                }`}
              >
                {sv.service}
              </button>
            ))}
          </div>
        </Modal>
      )}

      {uninstalling && (
        <UninstallDialog
          onClose={() => setUninstalling(false)}
          onDone={() => {
            setUninstalling(false)
            // 删完回到欢迎页：状态机重新判一次（Rust 侧的 install.done 已经被清掉了）
            send({ type: 'UNINSTALLED' })
          }}
        />
      )}

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
