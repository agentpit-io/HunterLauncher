/**
 * 前后端共用的数据形状。Rust 侧的 serde 结构按这个对齐（统一 camelCase 重命名）。
 * M2 起这些结构承载的全部是**真实数据**：拿不到的字段就是 null，界面显示「—」加原因。
 */

export interface AppInfo {
  /** 启动器自身版本，来自 Cargo.toml 的 version */
  launcherVersion: string
  /** linux | windows | macos | 其它 */
  platform: string
  /** x86_64 | aarch64 | … */
  arch: string
  /** native = 用系统原生标题栏（macOS 红绿灯）；custom = 自绘三个圆点 */
  windowChrome: 'native' | 'custom'
}

/** 启动时问一次：装过没有、在跑没有。决定直接进运行面板还是走向导。 */
/** 打开启动器该进哪一页（I12 · R1 的五行表）。 */
export type BootRoute = 'welcome' | 'dashboard' | 'data-found'

export interface BootState {
  installed: boolean
  running: boolean
  locale: string
  hunterTag: string
  workDir: string
  /** 该进哪一页。**以现状为准**，不只看 install.done（I12 · R1） */
  route: BootRoute
  posture: Posture
  /** 一句人话，来自后端的实测复查 */
  headline: string
  /** 这一次有没有替用户补写安装标记 */
  adopted: boolean
  launcherVersion: string
  installedAt: string
  lastHealthyAt: string
  /** route === 'data-found' 时，本项目名下还剩几个数据卷 */
  volumeCount: number
  elapsedMs: number
}

// ── 资源监控（I12 · R2）───────────────────────────────────────────────────

export type MonitorLevel = 'ok' | 'warn' | 'crit'

export interface HostMetrics {
  cpuPct: number | null
  cpuCores: number | null
  memUsedBytes: number | null
  memTotalBytes: number | null
  memPressure: string | null
  memPressureLevel: MonitorLevel
  diskFreeBytes: number | null
  diskTotalBytes: number | null
  diskMount: string | null
  diskLevel: MonitorLevel
  /** 拿不到的每一项都在这里配一句原因（红线 1） */
  reasons: Record<string, string>
}

export interface RuntimeMetrics {
  applicable: boolean
  reason: string
  cpus: number | null
  memTotalBytes: number | null
  memUsedBytes: number | null
  memLevel: MonitorLevel
  diskUsedBytes: number | null
  diskTotalBytes: number | null
  diskLevel: MonitorLevel
  reasons: Record<string, string>
}

export interface ServiceUsage {
  service: string
  cpuPct: number | null
  memBytes: number | null
  memLimitBytes: number | null
  restartCount: number | null
  oomKilled: boolean | null
}

export interface ServiceMetrics {
  services: ServiceUsage[]
  reason: string
}

export interface VolumeInfo {
  name: string
  short: string
  labelMissing: boolean
  mountpoint: string
  sizeBytes: number | null
}

export interface StorageMetrics {
  volumes: VolumeInfo[]
  volumesTotalBytes: number | null
  dbBytes: number | null
  imagesBytes: number | null
  imagesFound: number
  reasons: Record<string, string>
}

// ── 重装前检测已有数据（I12 · R5）─────────────────────────────────────────

export type DataDecision = 'fresh' | 'reuse' | 'missing-secrets' | 'downgrade' | 'backup-only'

export interface DataCheck {
  decision: DataDecision
  volumes: VolumeInfo[]
  hasDb: boolean
  hasSecrets: boolean
  hasJwtSecret: boolean
  pgVersion: string | null
  targetPgVersion: string | null
  migrationMax: string | null
  targetMigrationMax: string | null
  tableCount: number | null
  lastWrite: string | null
  backups: number
  deep: boolean
  deepSkipped: string | null
  headline: string
  lines: string[]
  elapsedMs: number
}

// ── 停止 / 启动 / 重启（I12 · R3）─────────────────────────────────────────

export interface StackPlan {
  builtinRunning: boolean
  builtinMemGb: number
  services: string[]
}

export interface StackOpResult {
  headline: string
  steps: string[]
  ready: number
  total: number
  elapsedMs: number
}

export type DockerRuntimeKind =
  | 'docker-engine'
  | 'docker-desktop'
  | 'orbstack'
  | 'colima'
  | 'podman'
  | 'unknown'

export interface GuideStep {
  text: string
  url: string | null
}

/** 三平台的安装引导。由 Rust 按当前平台给出，界面只负责渲染。 */
export interface InstallGuide {
  platform: string
  title: string
  steps: GuideStep[]
  /** Docker Desktop 的商业授权提示（方案 §6 要求） */
  licenseNote: string
}

/** 一个候选位置的探测结果（I4 的多路径探测）。 */
export type ProbeOutcome = 'found' | 'missing' | 'not-executable'

export interface ProbeStep {
  /** 配置 / PATH / 已知位置 */
  source: string
  candidate: string
  outcome: ProbeOutcome
}

export interface DockerInfo {
  installed: boolean
  daemonRunning: boolean
  runtime: DockerRuntimeKind
  runtimeLabel: string | null
  clientVersion: string | null
  serverVersion: string | null
  composeVersion: string | null
  arch: string | null
  /**
   * 实际用的 docker 可执行文件的绝对路径（I4）。
   * macOS 上同时装过 Docker Desktop 与 OrbStack 是常事，不写出来谁也说不清用的是哪个。
   */
  dockerPath: string | null
  /** 这条路径是从哪找到的：配置 / PATH / 已知位置 */
  dockerPathSource: string | null
  /** 软链指向哪里（/usr/local/bin/docker → OrbStack 的 xbin） */
  dockerLinkTarget: string | null
  /** 按顺序探过的每一个位置。没找到 docker 时界面会整份列出来 */
  dockerProbe: ProbeStep[]
  /** compose 是插件还是独立可执行文件 */
  composeMode: string | null
  /** Windows 专用；其它平台为 null */
  wsl: boolean | null
  meetsMinimum: boolean
  /** 不就绪时具体是哪一条不满足 */
  problem: string | null
  installGuide: InstallGuide | null
}

export interface QuotaInfo {
  usedToday: number
  limitDaily: number
  remaining: number
  /** ISO 8601，来自网关 */
  resetAt: string | null
  rpm: number | null
  concurrency: number | null
  /** 今天的额度用完了没有。来自网关的 `exhausted` 字段（读不到时按 remaining 兜底） */
  exhausted: boolean
}

export interface ModelAlias {
  id: string
  purpose: string
}

/**
 * key 校验结果。
 * M0 §1.3 实测：网关对「格式错 / 不存在 / 已吊销 / 未带 key」返回**字节级完全相同**的 401，
 * 所以这里只区分「本地就判得出的格式问题」「网关拒绝」「额度用尽」「网络不通」四种，
 * **不假装能认出「已吊销」**。
 */
export type KeyReason = 'ok' | 'malformed' | 'rejected' | 'exhausted' | 'network'

export interface KeyCheckResult {
  valid: boolean
  reason: KeyReason
  quota: QuotaInfo | null
  models: ModelAlias[]
  /** 给用户看的中文说明。valid 为 false 时是失败原因；
   *  valid 为 true 且 reason 是 exhausted 时，是「key 没问题但今天额度用完了」的提醒 */
  message: string | null
  /** 对应错误码，界面据此决定跳不跳错误页 */
  code: string | null
}

/** 自带 key 模式的真实连通性检查结果。 */
export interface OwnKeyCheck {
  ok: boolean
  /** 真跑通的那条路径：models | chat | gateway */
  via: string | null
  message: string
  /** 是不是自动开了 LLM_SCHEMA_SANITIZE（DeepSeek） */
  schemaSanitize: boolean
  models: string[]
}

export interface RegistryProbe {
  id: string
  label: string
  prefix: string
  /** manifest 真的探到了才是 true */
  available: boolean
  elapsedMs: number
  detail: string | null
}

export type PullState = 'pending' | 'downloading' | 'extracting' | 'done' | 'failed'
/** preparing = 选源 / 取 compose / 写配置；这一段没有字节进度，界面显示步骤文字 */
export type PullPhase = 'preparing' | 'pulling' | 'done' | 'failed'

export interface ImagePull {
  /** compose 里的服务名 */
  service: string
  /** 完整镜像引用，例如 ghcr.io/agentpit-io/hunter-community-web:1.2.0 */
  ref: string
  /** 去掉 registry 与 tag 的短名，界面左列显示这个 */
  shortRef: string
  tag: string
  totalBytes: number
  downloadedBytes: number
  state: PullState
  /** manifest 没读到，分母是观测值而不是真值 */
  sizeUnknown: boolean
  /** 这个镜像从第一个字节到拉完花了多少秒；没拉完就是 null */
  seconds: number | null
}

/** 与 `src/state/machine.ts` 的 ErrorCode 对齐；这里用宽松的 string 避免循环依赖。 */
export type ErrorCodeLike = string

export interface PullProgress {
  phase: PullPhase
  registry: string
  registryLabel: string
  images: ImagePull[]
  totalBytes: number
  downloadedBytes: number
  percent: number
  speedBps: number | null
  etaSeconds: number | null
  /** 第几次尝试（1 起），换源重试时会增加 */
  attempt: number
  /** 最近若干行原始进度日志，视觉稿第 2 张底部那个框 */
  log: string[]
  error: string | null
  /**
   * 出错时的**真实错误码**。I4 之前没有这个字段，前端把安装阶段的任何失败
   * 都当成 `E_PULL_FAILED`，于是「项目名被另一个工作目录占着」会显示成
   * 「镜像拉取失败」（标题与正文互相打架，诊断助手也据此给错建议）。
   */
  errorCode: ErrorCodeLike | null
}

export type Health = 'healthy' | 'starting' | 'unhealthy' | 'none' | 'pending'

export interface ServiceStatus {
  service: string
  state: string
  health: Health
  /** 宿主端口；llm-shim 不发布端口时为 null（M0 §5.3 的坑 3） */
  port: number | null
  /**
   * 这个端口**实际**绑在哪个地址（docker 自己报的 `Publishers[].URL`，不是我们写的配置）。
   * 读不到就是 null —— 界面显示「还没读到」，不猜（红线 1）。
   */
  bind: string | null
  exitCode: number | null
}

export interface EnvRow {
  key: string
  value: string | null
  /** value 为 null 时说明为什么拿不到（红线 1：拿不到要给原因） */
  reason?: string
}

/** 从本机 api 读回来的那几项。读不到就是 reachable=false + reason（红线 1）。 */
export interface UpstreamFacts {
  reachable: boolean
  apiKeyConfigured: boolean | null
  /** env | db | none */
  llmSource: string | null
  llmModel: string | null
  builtinQuota: boolean | null
  dataSupplyConfigured: boolean | null
  reason: string | null
}

/** 上游确实没有、因此界面只能显示「—」的接口。与成果文档「需上游配合」同一份清单。 */
export interface MissingEndpoint {
  id: string
  endpoint: string
}

export interface RuntimeStatus {
  hunterTag: string | null
  /** 今日额度，来自网关的 /api/saas/llm/quota；拿不到就是 null */
  quota: QuotaInfo | null
  latestTag: string | null
  running: boolean
  uptimeSeconds: number | null
  webUrl: string | null
  /**
   * 网页端口现在是不是**不止本机**能打开。
   *
   * 免费版只允许本机访问（用户 2026-09-21 19:05 的决定），所以新装的机器这一项恒为 false。
   * 为 true 的唯一情形是「这台机器升级前就对局域网开放，升级时有意没动它」——
   * 那时运行面板出一行提示 + 一个「只允许本机访问」按钮（收紧是单向的）。
   */
  webLanExposed: boolean
  services: ServiceStatus[]
  env: EnvRow[]
  /** 运行面板中间那个日志框 */
  log: string[]
  /** api 的 /api/health 自报 key 有没有落进容器；读不到为 null（M0 §3.1） */
  apiKeyConfigured: boolean | null
  installed: boolean
  upstream: UpstreamFacts
  /** 「数据源」卡片的主值；null 时界面显示「—」 */
  dataSource: string | null
  dataSourceSub: string
  missing: MissingEndpoint[]
}

export interface LauncherSettings {
  locale: string
  autostart: boolean
  checkUpdate: boolean
  registry: string
  hunterTag: string
  workDir: string
  telemetry: boolean
  /** 空 = 暂未开启上报。界面按这个如实写文案（红线 1） */
  telemetryEndpoint: string
  /** gateway | own */
  modelMode: string
  modelBaseUrl: string
  modelName: string
  registryPrefix: string
  /**
   * 网页端口现在是不是**不止本机**能打开。**只读**。
   *
   * 免费版只允许本机访问（用户 2026-09-21 19:05 的决定，待办池 P1-20 已关闭），
   * 设置页里没有开关，只有一行说明「局域网访问为付费版功能」。
   * 为 true 时设置页与运行面板各给一个「只允许本机访问」按钮（单向）。
   */
  webLanExposed: boolean
  /** AI 诊断助手。默认开；关掉之后只用确定性规则，一个 token 也不花（I4） */
  assist: boolean
  /** I5 授权档位：auto 自动驾驶 / confirm 逐步确认 / off 关闭 */
  assistMode: AssistMode
  /** 用户是哪一刻做的授权（上海时间）。空 = 还没授权过 */
  assistConsentedAt: string
  /** 现在实际用的 docker 路径。只读，拿不到就是 null（界面显示「—」） */
  dockerPath: string | null
  /** I7：授权页上那一项勾 —— 没有 Docker 时允许 AI 自动装一套 */
  allowInstallRuntime: boolean
  /** I7：没有 Docker 时走哪条路。builtin（默认，零点击）/ orbstack（备选） */
  installRoute: string
  /** I7：内置运行时装了没有（只读，当场探的） */
  builtinRuntimeInstalled: boolean
  /** I7：内置运行时的虚拟机在跑没有（只读） */
  builtinRuntimeRunning: boolean
  /** I7：现在在管理哪一套别人的 Hunter。空 = 没有接管（只读） */
  takeoverProject: string
}

// ── I7 · 内置运行时 ──────────────────────────────────────────────────────

export interface BuiltinRuntimeItem {
  component: string
  version: string
  file: string
  sha256: string
  bytes: number
  /** 实际从哪个源下的：github / tencent-hk / cache */
  source: string
}

export interface BuiltinRuntimeStatus {
  /** 这个平台支持内置运行时吗 */
  supported: boolean
  /** 不支持的原因（支持时为空）。**界面上原样显示，不要自己改写** */
  unsupportedReason: string
  installed: boolean
  running: boolean
  profile: string
  dir: string
  socket: string
  items: BuiltinRuntimeItem[]
  installedAt: string
  /** 清单里这台机器要下多少字节（真实值，来自写死的清单） */
  downloadBytes: number
}

// ── I7 · 接管本机已有的那一套 Hunter ─────────────────────────────────────

export interface TakeoverCandidate {
  project: string
  containers: string[]
  ports: number[]
  /** 读不到就是空 —— 那种情况只能只读监控 */
  workingDir: string
  configFiles: string
  /** 猜不出来就是 0，界面显示「—」 */
  webPort: number
}

export interface TakeoverContainerLine {
  name: string
  status: string
  image: string
  ports: string
}

export interface TakeoverState {
  active: boolean
  /** 有 compose 文件才管得起来；否则只能看 */
  manageable: boolean
  project: string
  workingDir: string
  since: string
  webUrl: string
  containers: TakeoverContainerLine[]
  /** 读不到状态时的原因。**界面要显示它**，不能只留一片空白 */
  note: string
}

export type TakeoverOp = 'stop' | 'start' | 'restart'

// ── I7 · 一键反馈直达 ────────────────────────────────────────────────────

export interface OneClickFeedback {
  bundlePath: string
  bundleBytes: number
  issueUrl: string
  issueTitle: string
  issueBody: string
  /** 出口闸命中的那一处；null = 干净 */
  scanHit: string | null
  note: string
}

// ── AI 诊断助手（I4 §三） ────────────────────────────────────────────────

export type ActionKind = 'readOnly' | 'mutating'

/** 一个**将要执行**的动作。command 就是原样展示给用户看的那一条。 */
export interface ActionPlan {
  id: string
  kind: ActionKind
  /** 要做什么 */
  title: string
  /** 为什么要做 */
  why: string
  /** 完整命令的参数数组。不跑子进程的动作是空数组，看 summary */
  argv: string[]
  summary: string | null
}

/** 第一层（确定性规则）给的结论。 */
export interface RuleSuggestion {
  rule: string
  code: string | null
  title: string
  detail: string
  actions: ActionPlan[]
  /** 为真时不显示「让 AI 帮我看看」主按钮 —— 规则层已经有把握了 */
  confident: boolean
}

export interface RanAction {
  id: string
  title: string
  command: string
  ok: boolean
  output: string
}

/** 被拒掉的动作（白名单之外或参数不合法）。**要显示出来**。 */
export interface RejectedAction {
  name: string
  reason: string
}

export interface AssistTurn {
  round: number
  text: string | null
  ran: RanAction[]
  pending: ActionPlan[]
  rejected: RejectedAction[]
  /** 网关返回的 usage.total_tokens；没给就是 null */
  tokens: number | null
}

export type DegradeReason =
  | 'disabled'
  | 'no-key'
  | 'offline'
  | 'gateway-error'
  | 'quota-exhausted'
  | 'rate-limited'
  | 'timeout'
  | 'rounds-exhausted'

export interface Degraded {
  reason: DegradeReason
  message: string
}

export interface AssistState {
  enabled: boolean
  hasKey: boolean
  rule: RuleSuggestion
  turns: AssistTurn[]
  pending: ActionPlan[]
  totalTokens: number
  rounds: number
  maxRounds: number
  degraded: Degraded | null
  done: boolean
  /** 整份诊断报文（已脱敏）。「复制诊断信息」用的就是它 */
  reportText: string
  /**
   * 现在可以接着往下装了（I9 的 P0-3）：docker 好了，而这一套还没装完。
   * 后端已经自己把安装跑起来了，界面只要回到过程流那一页。
   */
  canResumeInstall: boolean
  /** 全自动档下规则层**自己跑掉**的那几个动作。做了什么必须看得见 */
  autoRan: AssistAutoRan[]
}

/** 规则层自动执行过的一条。 */
export interface AssistAutoRan {
  id: string
  title: string
  ok: boolean
  text: string
}

/** 「查看本机将要发送的数据」。lines 就是 queue.jsonl 的原文。 */
export interface TelemetryView {
  enabled: boolean
  endpoint: string
  queuePath: string
  lines: string[]
  events: string[]
}

/** 诊断包里的一节。用户可以逐节勾掉不带（方案 §11.1）。 */
export interface DiagSection {
  id: string
  title: string
  /** 已经脱敏过的正文 */
  body: string
  defaultOn: boolean
  /** 这一节为什么是空的 */
  note: string | null
}

export interface FeedbackForm {
  /** deploy | result | feature | other */
  kind: string
  description: string
  contact: string
  errorCode: string
}

export interface ExportResult {
  path: string
  bytes: number
}

// ── M4 · 更新 / 升级 / 备份 / 离线包 ────────────────────────────────────

/** 启动器自己有没有新版本（方案 §10）。 */
export interface LauncherUpdate {
  available: boolean
  current: string
  version: string | null
  notes: string | null
  date: string | null
  /** 这台机器上能不能就地装：AppImage / Windows / macOS 能，.deb 不能 */
  canSelfInstall: boolean
  /** appimage | deb | windows | macos | unknown */
  installKind: string
  /** 查不到时的原因（红线 1：拿不到要给原因） */
  reason: string | null
}


/** Hunter 有没有新版本 + Release Notes 摘要。 */
export interface UpgradeCheck {
  current: string
  latest: string | null
  hasUpdate: boolean
  notes: string | null
  notesUrl: string | null
  publishedAt: string | null
  /** 跨大版本（方案 §10 第 4 条：要提示读升级须知） */
  majorJump: boolean
  reason: string | null
}

export interface UpgradeResult {
  ok: boolean
  from: string
  to: string
  backupId: string | null
  backupSqlBytes: number | null
  rolledBack: boolean
  message: string
}

export interface UpgradeStatus {
  running: boolean
  steps: string[]
  result: UpgradeResult | null
  error: string | null
}

/** 一次备份。`sqlBytes` 为 null 说明数据库那一半没做成，原因在 `sqlError`。 */
export interface BackupMeta {
  id: string
  tag: string
  at: string
  sqlBytes: number | null
  sqlError: string | null
  files: string[]
}

export interface MatchedImage {
  service: string
  reference: string
  bytes: number | null
}

/** 离线包导入的结果（方案 §9）。 */
export interface OfflineImport {
  path: string
  bytes: number
  seconds: number
  loaded: string[]
  matched: MatchedImage[]
  /** 还缺的服务名。非空就说明包不完整 */
  missing: string[]
  registryPrefix: string | null
  basePrefix: string | null
  tag: string | null
  /** 够不够跳过拉取 */
  complete: boolean
}

/** 统一的失败形状。所有 ipc 调用失败都变成这个，界面据此显示「—」和原因。 */
export interface IpcFailure {
  code: string
  message: string
}

export class IpcError extends Error {
  readonly code: string
  constructor(code: string, message: string) {
    super(message)
    this.name = 'IpcError'
    this.code = code
  }
}

// ── I5 · AI 自动驾驶安装（设计文档 §七） ─────────────────────────────────

/** 授权档位。存 `launcher.toml [assist] mode`。 */
export type AssistMode = 'auto' | 'confirm' | 'off'

export type AssistEventKind =
  | 'step'
  | 'issue'
  | 'analyze'
  | 'action'
  | 'verify'
  | 'review'
  | 'resolved'
  | 'needUser'
  | 'failed'
  | 'summary'

export type AssistEventStatus = 'running' | 'ok' | 'warn' | 'failed' | 'waiting' | 'skipped'

export interface AssistChoice {
  /** 回传给 assist_auto_answer 的值 */
  value: string
  /** 按钮文案。**是结论不是问句** */
  label: string
  primary: boolean
}

/** 过程流里的一条。按 `parent` 组成一棵树。 */
export interface AssistEvent {
  id: number
  parent: number | null
  kind: AssistEventKind
  status: AssistEventStatus
  title: string
  detail: string
  /** 「详情」折叠里的技术细节（已脱敏） */
  tech?: string[]
  tokens?: number
  elapsedMs?: number
  choices?: AssistChoice[]
  at: string
}

/** 顶上那一行常驻摘要。 */
export interface AssistSummary {
  solved: number
  open: number
  tokens: number
  rounds: number
  maxRounds: number
  elapsedMs: number
  phase: string
}

export interface AssistAutoSnapshot {
  running: boolean
  events: AssistEvent[]
  summary: AssistSummary
}

/** 一次自动安装的结果（`assist://done` 带的那一份）。 */
export interface AssistOutcome {
  ok: boolean
  code: string | null
  message: string | null
  solved: number
  rounds: number
  tokens: number
  elapsedMs: number
  url: string | null
  /**
   * 这一次**什么都没装**：开工前的复查发现那一套已经在跑了（I11 · U2）。
   * 界面据此把完成页那句话从「装好了」换成「本来就好着，没重装」。
   */
  reused?: boolean
}

// ── 现状复查（I11 · U1 / U2）──────────────────────────────────────────────

/** 复查的四种结论。和 Rust 侧 `selfcheck::Posture` 一一对应。 */
export type Posture = 'absent' | 'incomplete' | 'partial' | 'healthy'

/**
 * 「Hunter 现在到底在不在跑」的一次只读复查。
 *
 * 错误页靠它自己看见「其实已经好了」—— 0.1.9 在用户 Mac 上，
 * 服务在 41 分钟前就全绿了，界面却一直挂着那张失败卡片。
 */
export interface SelfCheckReview {
  posture: Posture
  services: ServiceStatus[]
  ready: number
  /** 应该有几个（后端给，前端不写死 6） */
  total: number
  unready: string[]
  missing: string[]
  webUrl: string | null
  /** 本机 GET 真实拿到的状态码；拿不到是 null（红线 1：不猜） */
  webStatus: number | null
  webReason: string | null
  /** 容器绑到本机之外的端口（U5） */
  drift: { service: string; port: number; actual: string }[]
  /** 一句人话的结论，界面直接显示 */
  headline: string
  /** 逐条证据（实测原话），折叠在「详情」里 */
  lines: string[]
  elapsedMs: number
}
