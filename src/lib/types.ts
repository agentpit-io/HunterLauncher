/** 前后端共用的数据形状。Rust 侧的 serde 结构按这个对齐（serde 用 camelCase 重命名）。 */

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

export type DockerRuntimeKind =
  | 'docker-engine'
  | 'docker-desktop'
  | 'orbstack'
  | 'colima'
  | 'podman'
  | 'unknown'

export interface DockerInfo {
  installed: boolean
  daemonRunning: boolean
  runtime: DockerRuntimeKind
  runtimeLabel: string | null
  clientVersion: string | null
  serverVersion: string | null
  composeVersion: string | null
  arch: string | null
  /** Windows 专用 */
  wsl: boolean | null
  meetsMinimum: boolean
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

export interface KeyCheckResult {
  valid: boolean
  /** 无效时的原因。M0 实测网关分不出「不存在 / 已吊销」，所以只有这三种。 */
  reason: 'ok' | 'malformed' | 'rejected' | 'network'
  quota: QuotaInfo | null
  models: ModelAlias[]
}

export type PullState = 'pending' | 'downloading' | 'extracting' | 'done' | 'failed'

export interface ImagePull {
  /** compose 里的服务名，视觉稿左列显示的就是它 */
  service: string
  /** 完整镜像引用，例如 ghcr.io/agentpit-io/hunter-community-web:1.2.0 */
  ref: string
  /** 去掉 registry 与 tag 的短名，界面左列显示这个 */
  shortRef: string
  /** 镜像 tag，显示在副标题里 */
  tag: string
  totalBytes: number
  downloadedBytes: number
  state: PullState
}

export interface PullProgress {
  registry: string
  images: ImagePull[]
  /** 最近若干行原始进度日志，视觉稿第 2 张底部那个框 */
  log: string[]
  etaSeconds: number | null
}

export type Health = 'healthy' | 'starting' | 'unhealthy' | 'none' | 'pending'

export interface ServiceStatus {
  service: string
  state: string
  health: Health
  /** 宿主端口；llm-shim 不发布端口时为 null（M0 §5.3 的坑 3） */
  port: number | null
}

export interface EnvRow {
  key: string
  value: string | null
  /** value 为 null 时说明为什么拿不到（红线 1：拿不到要给原因） */
  reason?: string
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
}

export interface LauncherSettings {
  locale: string
  autostart: boolean
  checkUpdate: boolean
  registry: string
  hunterTag: string
  workDir: string
  telemetry: boolean
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
