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
import { pullImageBytes, pullStateText, pullView } from '../lib/pull'
import { useStore } from '../state/context'
import type {
  BackupMeta,
  ImagePull,
  LauncherUpdate,
  Preflight,
  PullProgress,
  UpgradeStatus,
} from '../lib/types'

/**
 * 启动器的官网页面（下载页 + 版本说明）。
 *
 * 以前这里跳的是 GitHub 的 Release 页 —— 国内点开就是打不开的白页，
 * 而点这一下本来就是想看「新版本是什么、怎么装」。官网这一页同样有版本号、
 * 安装包大小、校验和与三个平台的下载按钮，而且国内直接可达。
 */
const LAUNCHER_PAGE = 'https://www.agentpit.io/hunter-launcher'

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
                onClick={() => void ipc.openExternal(LAUNCHER_PAGE)}
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
  /**
   * I18 · U5：**演示模式直接进「升级进行中」那一态。**
   *
   * 真机上 `st` 是点「升级」点出来的，而那一块（字节进度、网速、预计剩余）
   * 只有 `st.running` 为真才画得出来 —— 截图脚本点两下才出得来、还不一定点得中
   * （I17 的 `update-no-tag` 就是栽在这上面）。所以演示构建里由这三处直接给：
   * 这里、下面的 `runningRef`、以及 `ipc.upgradeStatus()` 的演示分支。
   * 做法与 `BackupPanel` 的 `backup-restore` 是同一个。
   */
  const demoingUpgrade = ipc.demoPage() === 'update-running' || ipc.demoPage() === 'update-retry'
  const [st, setSt] = useState<UpgradeStatus | null>(() =>
    demoingUpgrade ? { running: true, steps: [], result: null, error: null } : null,
  )
  const timer = useRef<number | undefined>(undefined)
  const d = c.data ?? null

  /**
   * U5：拉取进度。
   *
   * `EV_PULL` 是**每条**进度都发的，而升级本身很吃 CPU（拉镜像 + 解压）。
   * 所以这里分两步走，**上屏频率不高于每秒一次**：
   *
   * * 监听器只把最新的那一份**存进 ref**（不触发重渲）；
   * * 真正 `setState` 的是下面那个和轮询共用的一秒定时器。
   *
   * 直接一条一条 `setState` 会让整块面板每秒重渲几十次 —— 而那正是用户
   * 点「取消升级」那一刻最卡的时候（A5-5 要他在那时能看清已经下了多少）。
   */
  const pullRef = useRef<PullProgress | null>(null)
  const [pull, setPull] = useState<PullProgress | null>(null)
  /**
   * 明细默认折叠：六个镜像 × 状态 × 字节，摊开会把「还要多久」那一行挤下去。
   * `update-retry` 那一页例外 —— 它就是给 A5-3 / A5-4 出图的，明细要开着。
   */
  const [showImages, setShowImages] = useState(ipc.demoPage() === 'update-retry')
  /**
   * 升级在不在跑。**用 ref 而不是 `st.running`** —— 监听器是闭包里那一个，
   * 拿不到最新的 state；而「装 Docker 时也会发 EV_PULL」这件事必须挡住，
   * 否则用户开着这一页装 Docker，这里会显示成「正在升级」。
   */
  const runningRef = useRef(demoingUpgrade)

  // 升级要几分钟。开始之后每秒问一次进度，做完就停。
  useEffect(() => {
    return () => window.clearInterval(timer.current)
  }, [])

  // 演示模式那一页：进来就按「升级正在进行」跑起来（真机上这一步是点出来的）。
  // 这个一秒一次的定时器同时负责把 `pullRef` 里最新的那一份搬上屏（见 `poll`）
  useEffect(() => {
    if (demoingUpgrade) poll()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    let un: (() => void) | undefined
    let gone = false
    void ipc
      .onPullProgress((p) => {
        if (runningRef.current) pullRef.current = p
      })
      .then((f) => {
        if (gone) f()
        else un = f
      })
    return () => {
      gone = true
      un?.()
    }
  }, [])

  function poll() {
    window.clearInterval(timer.current)
    timer.current = window.setInterval(() => {
      void ipc.upgradeStatus().then((s) => {
        setSt(s)
        // 每秒把 ref 里最新那一份搬上屏；没有新样本就不动 state（不白重渲一次）
        setPull((cur) => (pullRef.current !== cur ? pullRef.current : cur))
        if (!s.running) {
          runningRef.current = false
          window.clearInterval(timer.current)
          setNonce((n) => n + 1)
        }
      })
    }, 1000)
  }

  /**
   * **点「升级」之后先体检，再决定要不要真升**（I17 · §4.2②）。
   *
   * 0.1.17 的顺序是「先写新 `.env` → 再拉镜像」——配置先改了才发现拉不动，
   * 客户 2026-09-29 那台的中间态就是这么留下的。现在这一步只探 manifest：
   * 当前源有这一版就直接升（**不额外多问**，A4-3）；没有就把「换到哪个源、要下多少」
   * 摆给用户，此刻 `.env` 与 `VERSION` **一个字节都还没动**。
   */
  async function preflightThenStart(tag: string) {
    setConfirm(false)
    setChecking(true)
    try {
      const pf = await ipc.upgradePreflight(tag)
      if (pf.currentOk) {
        await start(tag)
        return
      }
      setAsk(pf)
    } catch (e) {
      setSt({
        running: false,
        steps: [],
        result: null,
        error: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setChecking(false)
    }
  }

  async function start(tag: string, registry?: string) {
    setConfirm(false)
    setAsk(null)
    // 上一次升级的进度不许留到这一次（换了目标版本、换了源，那些数字全不作数了）
    pullRef.current = null
    runningRef.current = true
    setPull(null)
    setShowImages(false)
    setSt({ running: true, steps: [], result: null, error: null })
    try {
      await ipc.upgradeHunter(tag, registry)
      poll()
    } catch (e) {
      runningRef.current = false
      setSt({
        running: false,
        steps: [],
        result: null,
        error: e instanceof Error ? e.message : String(e),
      })
    }
  }

  const running = st?.running ?? false
  const [cancelling, setCancelling] = useState(false)
  /** 前置体检进行中（几秒的事，按钮上要说一句） */
  const [checking, setChecking] = useState(false)
  /** 体检发现「当前源还没有这一版」时的那一问（I17 · §4.2②） */
  const [ask, setAsk] = useState<Preflight | null>(null)

  /** 取消升级。后端收到之后会杀子进程、回滚配置，然后照常把结果写进 upgradeStatus。 */
  async function cancel() {
    setCancelling(true)
    try {
      await ipc.cancelInstall()
    } finally {
      // 取消之后后端还要写回配置、复核一遍，所以按钮一直灰到那一轮跑完为止
      window.setTimeout(() => setCancelling(false), 1500)
    }
  }

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
          {/*
            U5：**升级过程要看得见网速与剩余时间**（用户 2026-09-29 深夜提的）。
            在它之前，这一屏只有一行「正在升级…」—— 今早那位客户就是看不到
            「再等两分钟就好」，等了一分钟就点「一起停止」退出，把机器留在了中间态。
          */}
          {pull && <PullPanel p={pull} open={showImages} onToggle={() => setShowImages((v) => !v)} />}
          <LogBox lines={st.steps} className="mt-[8px] max-h-[160px]" emptyText={t.common.loading} />
          {st.error && (
            <div className="mt-[10px] text-sm leading-[1.6] text-danger" data-testid="upgrade-error">
              {st.error}
            </div>
          )}
          {st.result && (
            <div
              className={`mt-[10px] text-sm leading-[1.6] ${st.result.cancelled ? 'text-muted' : 'text-amber-text'}`}
              data-testid="upgrade-result"
            >
              {st.result.message}
            </div>
          )}
          {/* I16 · P1-1：升级过程中可以取消。
              点了就杀掉子进程、把配置写回升级前那一份，并明说容器一个都没动 ——
              客户 2026-09-25 那次只能强退，而强退发生在「配置已经改成新版本、
              镜像还没拉全」之后，**没有走到回滚**，机器就停在一个说不清的中间态。 */}
          {running && (
            <div className="mt-[10px] flex items-center gap-[10px]">
              <Button size="sm" data-testid="upgrade-cancel" disabled={cancelling} onClick={() => void cancel()}>
                {cancelling ? t.update.cancelling : t.update.cancelUpgrade}
              </Button>
              <span className="text-xs leading-[1.5] text-muted">{t.update.cancelHint}</span>
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
            disabled={running || checking}
            onClick={() => setConfirm(true)}
          >
            {running
              ? t.update.hunterUpgrading
              : checking
                ? t.update.preflightChecking
                : t.update.hunterUpgrade}
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
                disabled={checking}
                onClick={() => void preflightThenStart(d.latest!)}
              >
                {checking ? t.update.preflightChecking : t.update.hunterUpgrade}
              </Button>
            </>
          }
        >
          <p className="leading-[1.6]">{t.update.backupNote('~/.hunter/backups/')}</p>
          <p className="mt-[12px] leading-[1.6] text-muted">{t.update.rollbackNote}</p>
        </Modal>
      )}

      {/*
        I17 · §4.2②：**前置体检**发现「当前镜像源上还没有这一版」。
        这一刻 `.env` 与 `VERSION` 还是升级前那一份 —— 体检只探 manifest，什么都不改。
        两个出路：换到探得通的那个源继续（说清要下多少）、或者稍后再升（什么都不改）。
      */}
      {ask && (
        <Modal
          testId="upgrade-preflight"
          title={t.update.preflightTitle(ask.target)}
          onClose={() => setAsk(null)}
          footer={
            <>
              <Button size="sm" variant="ghost" data-testid="preflight-later" onClick={() => setAsk(null)}>
                {t.update.preflightLater}
              </Button>
              {ask.offer && (
                <Button
                  size="sm"
                  variant="primary"
                  data-testid="preflight-switch"
                  onClick={() => void start(ask.target, ask.offer!.id)}
                >
                  {t.update.preflightSwitch(ask.offer.label)}
                </Button>
              )}
            </>
          }
        >
          <p className="leading-[1.6]">{ask.reason ?? t.update.preflightTitle(ask.target)}</p>
          {ask.offer ? (
            <>
              <p className="mt-[12px] leading-[1.6]">
                {t.update.preflightOffer(ask.offer.label, ask.offer.prefix)}
              </p>
              <p className="tnum mt-[8px] leading-[1.6] text-muted">
                {ask.offerBytes === null
                  ? t.update.preflightSizeUnknown
                  : t.update.preflightSize(bytes(ask.offerBytes))}
              </p>
            </>
          ) : (
            <p className="mt-[12px] leading-[1.6] text-muted">{t.update.preflightNoOffer}</p>
          )}
          <p className="mt-[12px] leading-[1.6] text-muted">{t.update.preflightNothingChanged}</p>
        </Modal>
      )}
    </Card>
  )
}

// ── U5 · 升级过程中的字节进度 ─────────────────────────────────────────────

/**
 * U5：升级 / 安装过程中那一块字节进度。
 *
 * **判断「该显示什么」的那部分在 [`crate::pullView`]（`src/lib/pull.ts`）** ——
 * 那里是纯函数，三条诚实性红线（不编数字、总量未知时不写 0%、
 * 「本次已下载」必须用 `netBytes`）由 `src/lib/pull.test.ts` 逐条钉着。
 * 这个组件只负责把那些字符串摆出来。
 */
function PullPanel({ p, open, onToggle }: { p: PullProgress; open: boolean; onToggle: () => void }) {
  const { t } = useStore()
  const v = pullView(p, t.pull)

  return (
    <div
      className="mt-[10px] rounded-md border border-line bg-window px-3 py-2.5"
      data-testid="pull-progress"
    >
      <div className="tnum flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm text-body">
        <span data-testid="pull-size">{v.size}</span>
        {/* 分母没算出来时这里是 null —— **不写 0%** */}
        {v.percent && <span data-testid="pull-percent">{v.percent}</span>}
        {/* 网速取不到就不显示这一项 —— 不编 */}
        {v.speed && (
          <span className="text-muted" data-testid="pull-speed">
            {v.speed}
          </span>
        )}
        {/* 拿不到样本就是「剩余时间未知」，**不拿已用时去凑一个数** */}
        {v.eta && (
          <span className="text-muted" data-testid="pull-eta">
            {v.eta}
          </span>
        )}
        {/* 第几次尝试。今早那次客户就是第 1 次失败后换的源，而界面上什么都没说 */}
        {v.attempt && (
          <span className="text-amber-text" data-testid="pull-attempt">
            {v.attempt}
          </span>
        )}
      </div>

      {/* 「**这次**真的下载了多少」—— 必须用 netBytes（见 pull.ts 的红线 3） */}
      {v.net && (
        <div className="tnum mt-[6px] text-xs text-muted" data-testid="pull-net">
          {v.net}
        </div>
      )}

      {p.images.length > 0 && (
        <>
          <button
            type="button"
            className="mt-[8px] cursor-pointer text-xs text-amber-text underline-offset-2 hover:underline"
            data-testid="pull-images-toggle"
            onClick={onToggle}
          >
            {t.pull.detailsToggle}
          </button>
          {open && (
            <ul
              className="tnum mt-[6px] flex flex-col gap-[4px] text-xs text-dim"
              data-testid="pull-images"
            >
              {p.images.map((i: ImagePull) => (
                <li key={i.service} className="flex items-center justify-between gap-3">
                  <span className="min-w-0 truncate">
                    {t.pull.roles[i.service as keyof typeof t.pull.roles] ?? i.service}
                    <span className="ml-1.5 text-muted">{i.shortRef}</span>
                  </span>
                  <span className="shrink-0">
                    {pullStateText(i.state, t.pull)}
                    {' · '}
                    {pullImageBytes(i, t.pull)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
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
