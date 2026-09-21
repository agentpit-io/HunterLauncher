/*
 * 前端与 Rust 之间的唯一通道（技术方案附录 A：src/lib/ipc.ts）
 * ---------------------------------------------------------------------------
 * 两条路径：
 *   · 正常构建 → 真的 invoke 到 Tauri command。M2 起 Rust 侧全部是真实现：
 *     Docker 检测、key 校验、镜像源测速、拉镜像、写配置、起容器。
 *   · VITE_DEMO=1 的开发构建 → 走 demo.ts 的演示数据，界面右上角显示「演示数据」角标。
 *
 * 除了失败时抛 IpcError，这一层不做任何降级：拿不到就抛，由页面决定怎么显示「—」。
 * 绝不在这里用假数据兜底，那正是红线 1 禁止的事。
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { IpcError } from './types'
import type {
  AppInfo,
  AssistAutoSnapshot,
  AssistEvent,
  AssistMode,
  AssistOutcome,
  AssistState,
  AssistSummary,
  BackupMeta,
  BootState,
  BuiltinRuntimeStatus,
  DiagSection,
  DockerInfo,
  ExportResult,
  FeedbackForm,
  KeyCheckResult,
  LauncherSettings,
  LauncherUpdate,
  ManualInstall,
  MissingEndpoint,
  OfflineImport,
  OneClickFeedback,
  OwnKeyCheck,
  PullProgress,
  RegistryProbe,
  RuntimeStatus,
  ServiceStatus,
  TakeoverCandidate,
  TakeoverOp,
  TakeoverState,
  TelemetryView,
  UpgradeCheck,
  UpgradeStatus,
} from './types'
import * as demo from './demo'

/** 演示数据模式开关。发布构建里 VITE_DEMO 永远是空的（vite.config.ts 有构建期断言）。 */
export const DEMO = import.meta.env.VITE_DEMO === '1'

/** 演示模式下用初始化脚本注入的「直接打开某一页」，只有 DEMO 为真时才被采信。 */
declare global {
  interface Window {
    __HUNTER_DEMO_PAGE__?: string
  }
}

/**
 * 页面需要一个「初始就已填好」的演示态时从这里取（例如输入 key 页要显示已验证的样子）。
 * 发布构建里 DEMO 是编译期常量 false，整个表达式被摇成 null，demo.ts 不会进产物。
 */
export const demoSeed = DEMO
  ? { key: demo.DEMO_KEY, keyCheck: demo.demoKeyCheck, marker: demo.DEMO_MARKER }
  : null

export function demoPage(): string | null {
  if (!DEMO || typeof window === 'undefined') return null
  return window.__HUNTER_DEMO_PAGE__ ?? null
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args)
  } catch (e) {
    // Tauri 把 Rust 的 Err(String) 直接抛成字符串，约定格式是 `CODE: 说明`
    const raw = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e)
    const m = /^([A-Z][A-Z0-9_]+):\s*(.*)$/s.exec(raw)
    throw m ? new IpcError(m[1]!, m[2]!) : new IpcError('E_UNKNOWN', raw)
  }
}

// ── 基础信息 ─────────────────────────────────────────────────────────────

export async function appInfo(): Promise<AppInfo> {
  if (DEMO) return demo.demoAppInfo
  return call<AppInfo>('app_info')
}

export async function bootState(): Promise<BootState> {
  if (DEMO) return demo.demoBootState
  return call<BootState>('boot_state')
}

// ── 窗口控制 ─────────────────────────────────────────────────────────────

export async function windowMinimize(): Promise<void> {
  if (DEMO) return
  await call<void>('window_minimize')
}

export async function windowToggleMaximize(): Promise<void> {
  if (DEMO) return
  await call<void>('window_toggle_maximize')
}

export async function windowClose(): Promise<void> {
  if (DEMO) return
  await call<void>('window_close')
}

export async function openExternal(url: string): Promise<void> {
  if (DEMO) return
  await call<void>('open_external', { url })
}

// ── Docker ───────────────────────────────────────────────────────────────

export async function detectDocker(): Promise<DockerInfo> {
  if (DEMO) return demoPage() === 'docker-missing' ? demo.demoDockerMissing : demo.demoDocker
  return call<DockerInfo>('detect_docker')
}

/** Linux 上试着 systemctl start docker；mac/Windows 上会如实返回「要自己点」。 */
export async function startDaemon(): Promise<string> {
  if (DEMO) return '演示模式不真的启动 daemon'
  return call<string>('start_daemon')
}

// ── key 与模型 ───────────────────────────────────────────────────────────

export async function validateKey(key: string): Promise<KeyCheckResult> {
  if (DEMO) return demo.demoKeyCheck
  return call<KeyCheckResult>('validate_key', { key })
}

export interface ModelChoice {
  mode: 'gateway' | 'own'
  baseUrl?: string
  model?: string
  apiKey?: string
}

/** 选模型。自带 key 模式下 Rust 侧会真发一次请求做连通性检查。 */
export async function setModel(choice: ModelChoice): Promise<OwnKeyCheck> {
  if (DEMO) return demo.demoOwnKeyCheck
  return call<OwnKeyCheck>('set_model', { choice })
}

// ── 镜像源 ───────────────────────────────────────────────────────────────

export async function probeRegistries(tag?: string): Promise<RegistryProbe[]> {
  if (DEMO) return demo.demoProbes
  return call<RegistryProbe[]>('probe_registries', { tag: tag ?? null })
}

// ── 安装 ─────────────────────────────────────────────────────────────────

/** 启动一次安装。立刻返回，进度靠 onPullProgress 的事件推过来。 */
export async function startInstall(registryId?: string): Promise<void> {
  if (DEMO) return
  await call<void>('start_install', { registryId: registryId ?? null })
}

export async function cancelInstall(): Promise<void> {
  if (DEMO) return
  await call<void>('cancel_install')
}

export async function pullProgress(): Promise<PullProgress> {
  if (DEMO) return demo.demoPull
  return call<PullProgress>('pull_progress')
}

export async function startStack(): Promise<void> {
  if (DEMO) return
  await call<void>('start_stack')
}

export async function startingStatus(): Promise<RuntimeStatus> {
  if (DEMO) return { ...demo.demoRuntime, running: false, services: demo.demoStartingServices }
  return call<RuntimeStatus>('starting_status')
}

// ── 运行面板 ─────────────────────────────────────────────────────────────

export async function runtimeStatus(): Promise<RuntimeStatus> {
  if (DEMO) {
    return demoPage() === 'dashboard-quota-exhausted'
      ? demo.demoRuntimeQuotaExhausted
      : demo.demoRuntime
  }
  return call<RuntimeStatus>('runtime_status')
}

export async function stackAction(action: 'stop' | 'start' | 'restart' | 'down'): Promise<string> {
  if (DEMO) return '演示模式不真的操作容器'
  return call<string>('stack_action', { action })
}

export async function composeLogs(service?: string, tail = 200): Promise<string[]> {
  if (DEMO) return demo.demoComposeLog
  return call<string[]>('compose_logs', { service: service ?? null, tail })
}

export async function launcherLog(tail: number): Promise<string[]> {
  if (DEMO) return demo.demoLauncherLog
  return call<string[]>('launcher_log', { tail })
}

// ── 设置与诊断 ───────────────────────────────────────────────────────────

export async function readSettings(): Promise<LauncherSettings> {
  if (DEMO) return demo.demoSettings
  return call<LauncherSettings>('read_settings')
}

export async function writeSettings(settings: LauncherSettings): Promise<LauncherSettings> {
  if (DEMO) return settings
  return call<LauncherSettings>('write_settings', { settings })
}

/** 脱敏后的诊断文本。红线 2：里面不会有 key。 */
export async function diagnostics(): Promise<string> {
  if (DEMO) return '演示模式没有真实诊断信息'
  return call<string>('diagnostics')
}

/** 开机自启。返回**系统里真实的**状态，不是用户点的那个值。 */
export async function setAutostart(on: boolean): Promise<boolean> {
  if (DEMO) return on
  return call<boolean>('set_autostart', { on })
}

/**
 * 切换模型模式。Rust 侧会先做连通性检查，通过之后重写 .env 并重建
 * api / opencode / llm-shim 三个容器，再等它们健康。**这一步要几十秒。**
 */
export async function switchModel(choice: ModelChoice): Promise<string> {
  if (DEMO) return '演示模式不真的切换模型'
  return call<string>('switch_model', { choice })
}

// ── 遥测（本地队列，默认不上报） ─────────────────────────────────────────

export async function telemetryView(): Promise<TelemetryView> {
  if (DEMO) return demo.demoTelemetry
  return call<TelemetryView>('telemetry_view')
}

export async function telemetryClear(): Promise<string> {
  if (DEMO) return '演示模式不真的清空'
  return call<string>('telemetry_clear')
}

// ── 反馈与诊断包 ─────────────────────────────────────────────────────────

export async function diagnosticsSections(): Promise<DiagSection[]> {
  if (DEMO) return demo.demoDiagSections
  return call<DiagSection[]>('diagnostics_sections')
}

/** 导出 zip 到 ~/.hunter/diagnostics/。**不上传任何东西。** */
export async function exportDiagnostics(
  include: string[],
  form: FeedbackForm,
): Promise<ExportResult> {
  if (DEMO) return { path: '演示模式不真的导出', bytes: 0 }
  return call<ExportResult>('export_diagnostics', { include, form })
}

export async function feedbackIssueUrl(form: FeedbackForm): Promise<string> {
  if (DEMO) return 'https://github.com/agentpit-io/HunterLauncher/issues/new'
  return call<string>('feedback_issue_url', { form })
}

export async function exportLogs(
  source: 'launcher' | 'compose',
  service: string | undefined,
  tail: number,
): Promise<ExportResult> {
  if (DEMO) return { path: '演示模式不真的导出', bytes: 0 }
  return call<ExportResult>('export_logs', { source, service: service ?? null, tail })
}

/** 在文件管理器里定位导出的文件。只放行 ~/.hunter 里的路径。 */
export async function revealPath(path: string): Promise<void> {
  if (DEMO) return
  await call<void>('reveal_path', { path })
}

// ── 托盘与退出 ───────────────────────────────────────────────────────────

/** 触发一次托盘菜单动作，走的是和真点菜单完全相同的分支。 */
export async function trayInvoke(id: string): Promise<void> {
  if (DEMO) return
  await call<void>('tray_invoke', { id })
}

/** 退出。stopContainers=true 时先把容器停掉再退（方案 §5.8 的两条分支）。 */
export async function quitApp(stopContainers: boolean): Promise<void> {
  if (DEMO) return
  await call<void>('quit_app', { stopContainers })
}

// ── M4 · 启动器自更新（方案 §10） ────────────────────────────────────────

/** 查启动器自己有没有新版本。**不下载任何东西。** */
export async function checkLauncherUpdate(): Promise<LauncherUpdate> {
  if (DEMO) return demo.demoLauncherUpdate
  return call<LauncherUpdate>('check_launcher_update')
}

/**
 * 装启动器的新版本。
 *
 * 能就地装（AppImage / Windows / macOS）时这个 Promise **不会 resolve** ——
 * Rust 那边装完直接重启进程了。返回一个 ManualInstall 说明这台机器装不了自己
 * （.deb），包已经下好，要用户自己敲那条命令。
 */
export async function installLauncherUpdate(): Promise<ManualInstall | null> {
  if (DEMO) return null
  return call<ManualInstall | null>('install_launcher_update')
}

// ── M4 · Hunter 升级（方案 §5.6、§10） ───────────────────────────────────

/** force=true 绕过 6 小时缓存，用户亲手点「检查更新」时用。 */
export async function checkHunterUpdate(force = false): Promise<UpgradeCheck> {
  if (DEMO) return demo.demoHunterUpdate
  return call<UpgradeCheck>('check_hunter_update', { force })
}

/** 开始升级。立刻返回，进度靠 upgradeStatus() 轮询与 hunter://pull 事件。 */
export async function upgradeHunter(tag: string): Promise<void> {
  if (DEMO) return
  await call<void>('upgrade_hunter', { tag })
}

export async function upgradeStatus(): Promise<UpgradeStatus> {
  if (DEMO) return { running: false, steps: [], result: null, error: null }
  return call<UpgradeStatus>('upgrade_status')
}

// ── M4 · 备份 ───────────────────────────────────────────────────────────

export async function listBackups(): Promise<BackupMeta[]> {
  if (DEMO) return demo.demoBackups
  return call<BackupMeta[]>('list_backups')
}

/** 手动做一份备份（pg_dump + 配置）。要几秒到几十秒。 */
export async function createBackup(): Promise<BackupMeta> {
  if (DEMO) return demo.demoBackups[0]!
  return call<BackupMeta>('create_backup')
}

/** 把某次备份的数据库灌回去。**破坏性操作**，界面上要二次确认。 */
export async function restoreBackup(id: string): Promise<string> {
  if (DEMO) return '演示模式不真的恢复'
  return call<string>('restore_backup', { id })
}

// ── M4 · 离线包（方案 §9） ──────────────────────────────────────────────

/** 弹系统文件选择框。取消就返回 null。 */
export async function pickOfflineTar(): Promise<string | null> {
  if (DEMO) return null
  return call<string | null>('pick_offline_tar')
}

/** docker load 一个 tar。导入完整就会把镜像源与版本对齐到包里的那一套。 */
export async function importOffline(path: string): Promise<OfflineImport> {
  if (DEMO) return demo.demoOffline
  return call<OfflineImport>('import_offline', { path })
}

/** 六个镜像在本机齐了没有。拉取页靠它显示「已导入，跳过拉取」。 */
export async function offlineReady(): Promise<OfflineImport> {
  if (DEMO) return demo.demoOffline
  return call<OfflineImport>('offline_ready')
}

export async function missingEndpoints(): Promise<MissingEndpoint[]> {
  if (DEMO) return demo.demoMissing
  return call<MissingEndpoint[]>('missing_endpoints')
}

// ── 事件 ─────────────────────────────────────────────────────────────────

const EV_PULL = 'hunter://pull'
const EV_START = 'hunter://start'
const EV_LOG = 'hunter://log'
const EV_NAVIGATE = 'hunter://navigate'
const EV_QUIT_REQUEST = 'hunter://quit-request'
const EV_TRAY_ACTION = 'hunter://tray-action'
const EV_UPGRADE = 'hunter://upgrade'
const EV_LAUNCHER_UPDATE = 'hunter://launcher-update'

async function on<T>(name: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (DEMO) return () => {}
  return listen<T>(name, (e) => cb(e.payload))
}

export function onPullProgress(cb: (p: PullProgress) => void): Promise<UnlistenFn> {
  return on<PullProgress>(EV_PULL, cb)
}

export function onStartProgress(cb: (s: ServiceStatus[]) => void): Promise<UnlistenFn> {
  return on<ServiceStatus[]>(EV_START, cb)
}

// ── AI 诊断助手（I4 §三） ────────────────────────────────────────────────
//
// 两层的顺序在 Rust 侧定死（先规则、后 AI），前端只负责按返回值把该显示的显示出来。
// 演示模式下不调任何东西 —— 这一层会真的往网关发请求、真的动这台机器。

/** 第一层 · 确定性规则。零 token，任何时候都能调。 */
export async function assistDiagnose(ctx: {
  errorCode?: string
  errorMessage?: string
  stage?: string
}): Promise<AssistState> {
  if (DEMO) return demo.demoAssist
  return call<AssistState>('assist_diagnose', {
    errorCode: ctx.errorCode ?? null,
    errorMessage: ctx.errorMessage ?? null,
    stage: ctx.stage ?? null,
  })
}

/** 第二层 · 问一轮 AI。用不了时会在返回值的 degraded 里说明原因，**不抛错**。 */
export async function assistAsk(): Promise<AssistState> {
  if (DEMO) return demo.demoAssist
  return call<AssistState>('assist_ask')
}

/** 确认执行一个会改动机器的动作；执行完自动复验并接着问下一轮。 */
export async function assistConfirm(actionId: string, nextRound = true): Promise<AssistState> {
  if (DEMO) return demo.demoAssist
  return call<AssistState>('assist_confirm', { actionId, nextRound })
}

/** 执行规则层给的动作（不经过模型），执行完重新跑一遍规则。 */
export async function assistRuleAction(actionId: string): Promise<AssistState> {
  if (DEMO) return demo.demoAssist
  return call<AssistState>('assist_rule_action', { actionId })
}

export async function assistReset(): Promise<void> {
  if (DEMO) return
  await call<void>('assist_reset')
}

/** 准备阶段（选源 / 取 compose / 写配置）的逐行文字，拉取页底部的日志框显示它。 */
export function onLogLine(cb: (line: string) => void): Promise<UnlistenFn> {
  return on<string>(EV_LOG, cb)
}

/** 托盘让界面跳到某一页（logs / feedback / settings / dashboard）。 */
export function onNavigate(cb: (page: string) => void): Promise<UnlistenFn> {
  return on<string>(EV_NAVIGATE, cb)
}

/** 请求退出：容器还在跑，要问「保持后台运行 / 一起停止」（方案 §5.8）。 */
export function onQuitRequest(cb: () => void): Promise<UnlistenFn> {
  return on<boolean>(EV_QUIT_REQUEST, () => cb())
}

/** 托盘操作的结果，运行面板拿去显示一行提示。 */
export function onTrayAction(cb: (msg: string) => void): Promise<UnlistenFn> {
  return on<string>(EV_TRAY_ACTION, cb)
}

/** Hunter 升级的每一步文字。 */
export function onUpgradeStep(cb: (line: string) => void): Promise<UnlistenFn> {
  return on<string>(EV_UPGRADE, cb)
}

/** 后台每 24 小时查一次，查到启动器有新版本就发这个（方案 §10）。 */
export function onLauncherUpdate(cb: (u: LauncherUpdate) => void): Promise<UnlistenFn> {
  return on<LauncherUpdate>(EV_LAUNCHER_UPDATE, cb)
}

// ── I5 · AI 自动驾驶安装（设计文档 §二～八） ─────────────────────────────

export const EV_ASSIST = 'assist://event'
export const EV_ASSIST_SUMMARY = 'assist://summary'
export const EV_ASSIST_DONE = 'assist://done'

/**
 * 用户在一次授权页上做的选择。Rust 侧会写配置、写日志、写审计。
 *
 * `allowInstallRuntime` 是 I7 加的那一项勾（默认勾着）：电脑上没有 Docker 时，
 * 允许 AI 自动装一套到 `~/.hunter/runtime`。勾了之后那个动作就不再弹「需要你」。
 */
export async function assistConsent(
  mode: AssistMode,
  allowInstallRuntime = true,
): Promise<LauncherSettings> {
  if (DEMO) return demo.demoSettings
  return call<LauncherSettings>('assist_consent', { mode, allowInstallRuntime })
}

/** 开始一次全自动安装。立刻返回，过程走 `assist://event`。 */
export async function assistAutoStart(registryId?: string): Promise<void> {
  if (DEMO) return
  await call<void>('assist_auto_start', { registryId: registryId ?? null })
}

/** 回答「需要你」卡片。返回 false 表示这会儿并没有在等谁回答。 */
export async function assistAutoAnswer(value: string): Promise<boolean> {
  if (DEMO) return true
  return call<boolean>('assist_auto_answer', { value })
}

/** 页面刚挂载时先拉一份整棵树（事件可能在挂载之前就发过了）。 */
export async function assistAutoSnapshot(): Promise<AssistAutoSnapshot> {
  // 演示模式下按 __HUNTER_DEMO_PAGE__ 给两份不同的快照：
  // 正常直播 与「需要你」那张卡片各一张图
  if (DEMO)
    return demoPage() === 'auto-need-user' ? demo.demoAutoNeedUser : demo.demoAutoSnapshot
  return call<AssistAutoSnapshot>('assist_auto_snapshot')
}

/** 审计日志末尾若干条（设置页「AI 都做过什么」）。 */
export async function assistAuditTail(lines: number): Promise<string[]> {
  if (DEMO) return demo.demoAudit
  return call<string[]>('assist_audit_tail', { lines })
}

export function onAssistEvent(cb: (e: AssistEvent) => void): Promise<UnlistenFn> {
  return on<AssistEvent>(EV_ASSIST, cb)
}

export function onAssistSummary(cb: (s: AssistSummary) => void): Promise<UnlistenFn> {
  return on<AssistSummary>(EV_ASSIST_SUMMARY, cb)
}

export function onAssistDone(cb: (o: AssistOutcome) => void): Promise<UnlistenFn> {
  return on<AssistOutcome>(EV_ASSIST_DONE, cb)
}

// ── I7 · 内置运行时 / 接管 / 一键反馈 ────────────────────────────────────

/** 内置运行时现在什么情况（设置页）。只读。 */
export async function builtinRuntimeStatus(): Promise<BuiltinRuntimeStatus> {
  if (DEMO) return demo.demoBuiltinRuntime
  return call<BuiltinRuntimeStatus>('builtin_runtime_status')
}

/** 卸载内置运行时：删 colima 的 hunter profile + 清空 `~/.hunter/runtime`。 */
export async function builtinRuntimeUninstall(): Promise<string> {
  if (DEMO) return '演示数据：不会真的删东西'
  return call<string>('builtin_runtime_uninstall')
}

/** 本机上可以接管的那几套 Hunter（只读探测）。 */
export async function takeoverCandidates(): Promise<TakeoverCandidate[]> {
  if (DEMO) return demo.demoTakeoverCandidates
  return call<TakeoverCandidate[]>('takeover_candidates')
}

/** 当前接管态（运行面板）。 */
export async function takeoverState(): Promise<TakeoverState> {
  if (DEMO) return demo.demoTakeoverState
  return call<TakeoverState>('takeover_state')
}

/** 「直接用它，不再装一套」。 */
export async function takeoverAdopt(project: string): Promise<string> {
  if (DEMO) return '演示数据'
  return call<string>('takeover_adopt', { project })
}

/** 撤回接管，回到「自己装一套」。不动被接管的那一套一个字节。 */
export async function takeoverRelease(): Promise<string> {
  if (DEMO) return '演示数据'
  return call<string>('takeover_release')
}

/**
 * 对被接管那一套做 stop / start / restart。
 *
 * **`confirmed` 为 false 时 Rust 一定会拒绝** —— 界面拿 `takeoverConfirmText`
 * 把那句话弹给用户，他点了「确定」才用 `confirmed: true` 再调一次。
 */
export async function takeoverOp(op: TakeoverOp, confirmed: boolean): Promise<string> {
  if (DEMO) return '演示数据'
  return call<string>('takeover_op', { op, confirmed })
}

/** 二次确认要给用户看的那句话（文案由 Rust 给，界面不另写一套）。 */
export async function takeoverConfirmText(op: TakeoverOp): Promise<string> {
  if (DEMO) return '演示数据：真要停它吗？'
  return call<string>('takeover_confirm_text', { op })
}

/** 被接管那一套的日志（只读，已脱敏）。 */
export async function takeoverLogs(service?: string, lines = 200): Promise<string[]> {
  if (DEMO) return demo.demoComposeLog
  return call<string[]>('takeover_logs', { service: service ?? null, lines })
}

/**
 * 一键反馈：生成脱敏诊断包 + 预填 issue。
 *
 * **它不发送任何东西**，也不打开浏览器 —— 那是用户看过内容之后另一次点击的事。
 */
export async function feedbackOneClick(
  errorCode?: string,
  errorMessage?: string,
): Promise<OneClickFeedback> {
  if (DEMO) return demo.demoOneClick
  return call<OneClickFeedback>('feedback_one_click', {
    errorCode: errorCode ?? null,
    errorMessage: errorMessage ?? null,
  })
}
