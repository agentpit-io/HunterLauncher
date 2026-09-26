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
  BackupSettings,
  BootState,
  BuiltinRuntimeStatus,
  DataCheck,
  HostMetrics,
  RuntimeMetrics,
  ServiceMetrics,
  StackOpResult,
  StackPlan,
  StorageMetrics,
  CleanupDone,
  CleanupPlan,
  DiagSection,
  MonitorAlerts,
  RestorePreflight,
  RestoreReport,
  ScheduleStatus,
  UninstallOptions,
  UninstallPlan,
  UninstallReport,
  DockerInfo,
  ExportResult,
  FeedbackForm,
  KeyCheckResult,
  KeptKey,
  LauncherSettings,
  LauncherUpdate,
  MissingEndpoint,
  OfflineImport,
  OneClickFeedback,
  SelfCheckReview,
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
  LogshipPreview,
  LogshipOutcome,
  InterruptedUpgrade,
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

/**
 * 把窗口收起来回到托盘（I11 · U3）。**不退出进程、不动正在跑的服务。**
 * 错误页上那个「关闭」走的是它，不是 `windowClose`（那个等于请求退出）。
 */
export async function windowHide(): Promise<void> {
  if (DEMO) return
  await call<void>('window_hide')
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

/** 上次「只删应用」保留下来的那把 key 还能不能用（I14 · F3）。 */
export async function keptKey(): Promise<KeptKey> {
  if (DEMO) return demo.demoKeptKey(demoPage())
  return call<KeptKey>('kept_key')
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
    const p = demoPage()
    if (p === 'dashboard-quota-exhausted') return demo.demoRuntimeQuotaExhausted
    // I7：升级上来的老机器 —— 网页端口还绑在所有网卡上
    if (p === 'dashboard-lan') return demo.demoRuntimeLanExposed
    return demo.demoRuntime
  }
  return call<RuntimeStatus>('runtime_status')
}

export async function stackAction(action: 'stop' | 'start' | 'restart' | 'down'): Promise<string> {
  if (DEMO) return '演示模式不真的操作容器'
  return call<string>('stack_action', { action })
}

// ── 停止 / 启动 / 重启（I12 · R3）─────────────────────────────────────────

/** 按按钮**之前**问一句：这台机器要不要显示「顺便停运行环境」那个勾选。 */
export async function stackPlan(): Promise<StackPlan> {
  if (DEMO) return demo.demoStackPlan
  return call<StackPlan>('stack_plan')
}

/**
 * 停止 / 启动 / 重启。
 *
 * - `alsoRuntime`：停止时顺带停掉内置运行时的虚拟机（释放内存）；
 * - `service`：只重启这一个服务（不给就是六个一起）。
 */
export async function stackOp(
  action: 'stop' | 'start' | 'restart',
  opts: { alsoRuntime?: boolean; service?: string } = {},
): Promise<StackOpResult> {
  if (DEMO) return demo.demoStackOp(action)
  return call<StackOpResult>('stack_op', {
    action,
    alsoRuntime: opts.alsoRuntime ?? false,
    service: opts.service ?? null,
  })
}

/** 操作过程中的一步（后端每做完一件事推一条）。 */
export function onStackStep(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>('hunter://stack', (e) => cb(e.payload))
}

// ── 资源监控（I12 · R2）───────────────────────────────────────────────────
//
// 四条各自独立，因为**刷新频率不一样**（方案 R2 的表）：
// 这台电脑 5 秒、运行环境 30 秒、各服务 10 秒、数据卷与镜像 5 分钟。
// 合成一条命令的话最慢的那一层会把最快的那一层拖住。

export async function monitorHost(): Promise<HostMetrics> {
  if (DEMO) return demo.demoHost
  return call<HostMetrics>('monitor_host')
}

export async function monitorRuntime(): Promise<RuntimeMetrics> {
  if (DEMO) return demo.demoRuntimeMetrics
  return call<RuntimeMetrics>('monitor_runtime')
}

export async function monitorServices(): Promise<ServiceMetrics> {
  if (DEMO) return demo.demoServiceMetrics
  return call<ServiceMetrics>('monitor_services')
}

export async function monitorStorage(): Promise<StorageMetrics> {
  if (DEMO) return demo.demoStorage
  return call<StorageMetrics>('monitor_storage')
}

// ── 重装前检测已有数据（I12 · R5）─────────────────────────────────────────

/** 浅查：不起任何容器，一个字节都不写。 */
export async function dataCheck(): Promise<DataCheck> {
  if (DEMO) return demo.demoDataCheck
  return call<DataCheck>('data_check')
}

/** 深查：起一个用完就删的 postgres，把表数与迁移版本问出来（十几秒）。 */
export async function dataCheckDeep(): Promise<DataCheck> {
  if (DEMO) return demo.demoDataCheckDeep
  return call<DataCheck>('data_check_deep')
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

/**
 * 一键把网页端口收回本机（I7）。**单向** —— 收紧之后没有任何入口能放开，
 * 局域网访问是付费版功能。只有「升级前就对局域网开放」的老机器会看到这个按钮。
 */
export async function tightenWebBind(): Promise<string> {
  if (DEMO) return '演示模式不真的改端口绑定'
  return call<string>('tighten_web_bind')
}

export async function readSettings(): Promise<LauncherSettings> {
  if (DEMO)
    return demoPage() === 'settings-lan'
      ? { ...demo.demoSettings, webLanExposed: true }
      : demo.demoSettings
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
 * 装启动器的新版本。**两条路都由启动器自己装完**（I8）。
 *
 * 成功时这个 Promise **不会 resolve** —— Rust 那边装完直接重启进程了。
 * `.deb` 那条路会先弹一次 polkit 的原生授权框；用户在那个框上点取消、
 * 或者这台机器上没有 polkit，都会 reject 并带上原话。
 */
export async function installLauncherUpdate(): Promise<void> {
  if (DEMO) return
  return call<void>('install_launcher_update')
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

/**
 * 手动做一份备份（`pg_dump -Fc` + 密钥卷 + 配置，做完校验）。要几秒到几十秒。
 */
export async function createBackup(): Promise<BackupMeta> {
  if (DEMO) return demo.demoBackups[0]!
  return call<BackupMeta>('create_backup')
}

/**
 * 从一份备份恢复。**破坏性操作**：`confirm` 必须逐字是「恢复数据」，
 * Rust 那一侧也会再核一次（界面有 bug 也不该把数据覆盖掉）。
 */
export async function restoreBackup(id: string, confirm: string): Promise<RestoreReport> {
  if (DEMO) throw new IpcError('E_NOT_IMPLEMENTED', '演示模式不真的恢复')
  return call<RestoreReport>('restore_backup', { id, confirm })
}

// ── I13 · R6 备份与恢复 ───────────────────────────────────────────────────

export async function readBackupSettings(): Promise<BackupSettings> {
  if (DEMO) return demo.demoBackupSettings
  return call<BackupSettings>('read_backup_settings')
}

/** 存设置**并**当场把系统里的定时任务改成一致的样子。 */
export async function writeBackupSettings(settings: BackupSettings): Promise<BackupSettings> {
  if (DEMO) return settings
  return call<BackupSettings>('write_backup_settings', { settings })
}

/** 定时任务的现状（现查系统，不读配置里的备忘）。 */
export async function backupScheduleStatus(): Promise<ScheduleStatus> {
  if (DEMO) return demo.demoSchedule
  return call<ScheduleStatus>('backup_schedule_status')
}

export async function restorePreflight(id: string): Promise<RestorePreflight> {
  if (DEMO) return demo.demoPreflight
  return call<RestorePreflight>('restore_preflight', { id })
}

export async function restoreConfirmText(): Promise<string> {
  if (DEMO) return '恢复数据'
  return call<string>('restore_confirm_text')
}

/** 列任意目录里的备份（换电脑迁移：把备份拷过来，指给启动器看）。 */
export async function backupsInDir(dir: string): Promise<BackupMeta[]> {
  if (DEMO) return demo.demoBackups
  return call<BackupMeta[]>('backups_in_dir', { dir })
}

/** 弹系统目录选择框挑备份目录。取消就返回 null。 */
export async function pickBackupDir(): Promise<string | null> {
  if (DEMO) return null
  return call<string | null>('pick_backup_dir')
}

/** 备份 / 恢复过程中的一步。 */
export function onBackupStep(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>('hunter://backup', (e) => cb(e.payload))
}

// ── I13 · R4 删除应用 ─────────────────────────────────────────────────────

export async function uninstallPlan(scope: string, deep = false): Promise<UninstallPlan> {
  if (DEMO) return demo.demoUninstallPlan(scope)
  return call<UninstallPlan>('uninstall_plan', { scope, deep })
}

export async function uninstallConfirmText(scope: string): Promise<string> {
  if (DEMO) return scope === 'app-and-data' ? '删除应用和数据' : '删除应用'
  return call<string>('uninstall_confirm_text', { scope })
}

export async function uninstallRun(options: UninstallOptions): Promise<UninstallReport> {
  if (DEMO) throw new IpcError('E_NOT_IMPLEMENTED', '演示模式不真的删除')
  return call<UninstallReport>('uninstall_run', { options })
}

export function onUninstallStep(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>('hunter://uninstall', (e) => cb(e.payload))
}

// ── I13 · R7 异常监测与一键清理 ───────────────────────────────────────────

export async function monitorAlerts(): Promise<MonitorAlerts> {
  // 演示模式下**只有截图那一页**给提醒：别的页要是也顶着一条横幅，
  // 运行面板那张基准图就和真机上「一切正常」时的样子对不上了
  if (DEMO) {
    return demoPage() === 'dashboard-alert'
      ? demo.demoAlerts
      : { at: demo.demoAlerts.at, alerts: [], notify: [], reasons: {} }
  }
  return call<MonitorAlerts>('monitor_alerts')
}

export async function cleanupPlan(): Promise<CleanupPlan> {
  if (DEMO) return demo.demoCleanupPlan
  return call<CleanupPlan>('cleanup_plan')
}

export async function cleanupRun(): Promise<CleanupDone> {
  if (DEMO) throw new IpcError('E_NOT_IMPLEMENTED', '演示模式不真的清理')
  return call<CleanupDone>('cleanup_run')
}

/** 规则层判不了的那几条，交给诊断助手（会花 hunter 额度）。 */
export async function assistResourceAsk(alertId: string): Promise<string> {
  if (DEMO) throw new IpcError('E_NOT_IMPLEMENTED', '演示模式不问 AI')
  return call<string>('assist_resource_ask', { alertId })
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
  // I9：残骸那一页要显示的是另一条规则（以及「启动器已经替你做了这几步」那一块）
  if (DEMO) return demoPage() === 'error-builtin' ? demo.demoAssistBuiltin : demo.demoAssist
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
/** 「修好了，接着装」：后端已经把安装重新跑起来了，界面回到过程流那一页（I9）。 */
export const EV_ASSIST_RESUME = 'assist://resume'

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

/**
 * 现状复查：只读地问一句「Hunter 现在在不在跑」（I11 · U1）。
 *
 * `deep` 为真时多探一次「容器连不连得上模型网关」，要几秒到几十秒 ——
 * 界面上的轮询一律不开它。
 */
export async function selfCheck(deep = false): Promise<SelfCheckReview> {
  if (DEMO) return demo.demoSelfCheck(demoPage())
  return call<SelfCheckReview>('self_check', { deep })
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
  if (DEMO) {
    const p = demoPage()
    if (p === 'auto-need-user') return demo.demoAutoNeedUser
    if (p === 'auto-review') return demo.demoAutoReview
    if (p === 'auto-takeover-offer') return demo.demoAutoTakeoverOffer
    return demo.demoAutoSnapshot
  }
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

/**
 * 后端修好 Docker、自己把安装接着跑起来了（I9 的 P0-3）。
 *
 * 0.1.8 在用户 Mac 上缺的正是这一步：AI 14:54 把内置运行时起起来了，
 * 界面却还停在「没能自动装好」，`~/.hunter/app/` 到最后都是空的。
 */
export function onAssistResume(cb: () => void): Promise<UnlistenFn> {
  return on<unknown>(EV_ASSIST_RESUME, () => cb())
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
  // 演示模式下**只有截图脚本明确要那一页时**才说「正在接管」——
  // 否则每一张运行面板的截图都会变成接管态（接管是少数情况，不是默认）
  if (DEMO)
    return demoPage() === 'takeover'
      ? demo.demoTakeoverState
      : { ...demo.demoTakeoverState, active: false, project: '', containers: [] }
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
/**
 * **一键上传日志 · 第一步：看一眼要传什么。**
 *
 * 这一步**不发任何请求**。返回的 `body` 会被后端记下来，
 * 第二步 {@link logshipUpload} 传的就是它 —— 界面上看到的和送出去的
 * 是同一串字节，不存在「看到的是一套、发出去的是另一套」。
 */
export async function logshipPreview(opts?: {
  stage?: string
  errorCode?: string
  summary?: string
  include?: string[]
}): Promise<LogshipPreview> {
  if (DEMO) return demo.demoLogshipPreview
  return call<LogshipPreview>('logship_preview', {
    stage: opts?.stage ?? null,
    errorCode: opts?.errorCode ?? null,
    summary: opts?.summary ?? null,
    include: opts?.include ?? null,
  })
}

/**
 * **一键上传日志 · 第二步：真的传。**
 *
 * 传的是上一次 {@link logshipPreview} 给用户看过的那一份。
 * 没有预览过就会报错 —— 不给看就传等于替用户做决定。
 */
export async function logshipUpload(): Promise<LogshipOutcome> {
  if (DEMO) return demo.demoLogshipOutcome
  return call<LogshipOutcome>('logship_upload', {})
}

/**
 * 「上一次升级做完了没有」当场再查一遍（I16 · P1-2）。
 *
 * 演示模式下**只有 `dashboard-interrupted` 那一页**才给这张卡片 ——
 * 否则每一张运行面板的截图上都会顶着一条「上一次升级没做完」，
 * 而那在真机上是个罕见现场，不该出现在「一切正常」的那几张图里。
 */
export async function interruptedUpgrade(): Promise<InterruptedUpgrade | null> {
  if (DEMO) return demoPage() === 'dashboard-interrupted' ? demo.demoInterruptedUpgrade : null
  return call<InterruptedUpgrade | null>('interrupted_upgrade', {})
}

/**
 * **回退到正在跑的那一版**（中间态的第二个出口）。
 *
 * 只把配置写回去，不碰容器、不碰卷、不拉镜像。
 */
export async function revertToRunning(tag: string): Promise<string> {
  if (DEMO) return `已经回到 v${tag}（演示数据）`
  return call<string>('revert_to_running', { tag })
}

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
