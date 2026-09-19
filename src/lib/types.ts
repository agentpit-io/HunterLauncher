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

export interface DockerInfo {
  installed: boolean
  daemonRunning: boolean
  runtime: DockerRuntimeKind
  runtimeLabel: string | null
  clientVersion: string | null
  serverVersion: string | null
  composeVersion: string | null
  arch: string | null
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
  /** 给用户看的中文说明；valid 时为 null */
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
