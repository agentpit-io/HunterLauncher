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
export interface BootState {
  installed: boolean
  running: boolean
  locale: string
  hunterTag: string
  workDir: string
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
   * web 端口只允许本机访问。**默认 false** —— 也就是绑所有网卡，
   * 同一网络里的其他设备打开 `http://<这台机器的 IP>:<端口>` 就能直接用。
   *
   * 这是既定设计（总控规则红线 4 把 web 明确排除在「只绑 127.0.0.1」之外），
   * I2 只是把开关做出来，默认值没动 —— 默认该是哪一个由用户决定（待办池 P1-20）。
   */
  webLocalOnly: boolean
  /** AI 诊断助手。默认开；关掉之后只用确定性规则，一个 token 也不花（I4） */
  assist: boolean
  /** 现在实际用的 docker 路径。只读，拿不到就是 null（界面显示「—」） */
  dockerPath: string | null
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

/** `.deb` 这类装不了自己的格式：包下好了，给一条要用户自己敲的命令。 */
export interface ManualInstall {
  path: string
  bytes: number
  command: string
  message: string
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
