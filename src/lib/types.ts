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
  /** I16：上一次升级没做完（强退留下的中间态）。null = 没有这回事 */
  interruptedUpgrade: InterruptedUpgrade | null
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
  /** 其中有几份含密钥卷（I13 · R6）。有才提「从备份恢复密钥卷」那条路 */
  backupsWithSecrets: number
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

/** 上次「只删应用」保留下来的那把 key（I14 · F3）。完整的 key 不出 Rust 进程。 */
export interface KeptKey {
  present: boolean
  masked: string
  path: string
  check: KeyCheckResult | null
  /** `.env` 里留着 `HUNTER_API_KEY=…`，但它不是一把 hunter key 的样子（沿用不了，但要说一句） */
  badShape: boolean
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
  /**
   * 这一次**真的走了网络**的字节数。
   *
   * `downloadedBytes` 把「本机已有的层」也算进去了 —— 它是进度条的分子，用户关心的是
   * 整体完成度。但拿它去说「下载了 849 MB」就是在骗人：那 849 MB 一个字节都没过网。
   * 所以「这次下载了多少」这句话**必须**用它（I18 · U5 §3.3①；
   * Rust 侧同名字段的注释在 `compose.rs` 的 `PullProgress::net_bytes`）。
   */
  netBytes: number
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
  /** I16：这个容器**实际在用的镜像**。配置是意图，这一项才是现状（红线 1） */
  image: string | null
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
  /**
   * 「网页现在打得开吗」—— **面板大标题唯一的依据**（I17 · P0-1）。
   *
   * 0.1.17 及以前 Rust 侧算的是「任意一个服务在跑」，于是备份留下的一个孤儿
   * postgres 就能把大标题点亮成「Hunter 运行中」，还给一个指向死端口的按钮。
   * 现在它等于「web 在跑 **且** 本机 HTTP 探得通」。
   */
  running: boolean
  /**
   * 现状复查的结论（I17 · P0-1）。**和错误页、开机判定用的是同一个判定**
   * （Rust 侧 `selfcheck::Posture`，`Posture` 这个类型见本文件下半部分）。
   * 大标题在「在跑 / 没跑」两档里再按它细分（见 `Dashboard.tsx` 里的 `title`）。
   */
  posture: Posture
  /** 本机 GET `http://127.0.0.1:{web_port}/` **有没有拿到 HTTP 应答**（I17 · P0-1） */
  webOk: boolean
  /** **只在 `webOk` 为真时**才有值；否则是 `null`，界面不显示这一截（不是显示 0） */
  uptimeSeconds: number | null
  /** 只在 `webOk` 为真时给出 */
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
  /** 这一次安装收尾时发生的事（I14 · F4：定时备份任务挂没挂上） */
  postInstallNotes: string[]
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
  /**
   * F-02 三级授权：现在授权到哪几档（`["l1","l2"]`）。
   *
   * **默认值由 Rust 决定**（`config.rs` 的 `default_grants`：一级 + 二级，
   * 不含三级），界面照读，**不许在这里另写一份默认** —— 两份默认值迟早会分家。
   *
   * `null` = 这次不改它（`write_settings` 的语义：旧版界面不带这个字段，
   * 拿空表当「一档都不授权」会把安装能力关掉）。
   */
  assistGrants: string[] | null
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
  /** I16：**出错时自动上传日志**。默认 false，而且只在出错时才传 */
  autoUploadLogs: boolean
  /** I16：最近一次上传拿到的追踪码（只读）。空 = 从来没传过 */
  lastTraceCode: string
  /** I16：最近一次上传的时间，上海时间（只读） */
  lastUploadAt: string
}

// ── U-03 · 选盘 ─────────────────────────────────────────────────────────

/**
 * 一块盘。**含系统盘** —— 界面要能说清「为什么不是 C 盘」，就得先看到 C 盘。
 * 字段与 Rust 的 `runtime::disk::Volume` 逐字对应。
 */
export interface DiskVolume {
  /** 挂载点。Windows 上是 `D:\`，macOS / Linux 上是路径 */
  mount: string
  totalBytes: number
  freeBytes: number
  /** 系统卷。**永远不会被选为默认位置**（Rust 侧 `eligible` 里那一条） */
  isSystem: boolean
  /** 这块盘上已经有 Hunter 的内置运行时（沿用它，省一次搬家） */
  hasHunter: boolean
}

/** 「第一次装大约要占多少」里的一笔（键名对应 i18n 的 `disk.part.*`） */
export interface DiskPart {
  key: 'download' | 'images' | 'vmBase'
  bytes: number
}

/**
 * 选盘卡片要的全部**事实**（U-03）。数字全部来自 Rust 的真实探测或代码常量 ——
 * 界面只负责把这些数翻译成人话，一个数都不许自己写。
 */
export interface DiskPlan {
  volumes: DiskVolume[]
  /**
   * 打分第一名（挂载点）。`null` = **一块合格的都没有** ——
   * 这时界面必须有话可说，不许静默失败、更不许瞎选一块。
   */
  chosen: string | null
  /** 第一次装大约要占多少（GB，向上取整） */
  firstInstallGb: number
  parts: DiskPart[]
  /** 建议至少留多少（GB）：比「第一次装」多出一份备份与余量 */
  needGb: number
  /** 候选门槛：总容量小于这个数的一律不看（GB） */
  minTotalGb: number
  /**
   * 运行时磁盘**上限**（GB）。**与「实际已占」是两件事，不许混成一句** ——
   * I14·F2 曾把稀疏文件报成「86.1 GB」而实际只有 5.8 GB（差 15 倍）。
   * 量不到就是 `null`：界面如实说量不到，不编一个数。
   */
  capGb: number | null
  /** **实际已占**（GB）。量不到就是 `null` */
  usedGb: number | null
  /** 配置里现在写的 `[runtime] data_dir`（空 = 还没选过，用默认位置） */
  dataDir: string
  /** 现在**实际**落在哪 */
  runtimeDir: string
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

// ── 一键上传日志（I16 · P0-2） ───────────────────────────────────────────

/**
 * **将要上传的那一份**。`body` 就是真正送出去的字节 ——
 * 界面上给用户看的和发出去的是同一串，不存在两套。
 */
export interface LogshipPreview {
  /** 分节预览（和反馈页是同一套 Section、同一套脱敏） */
  sections: DiagSection[]
  body: string
  bodyBytes: number
  /** 客户端这一侧截过没有（服务端超 1 MB 还会再截一次，从头部截） */
  truncated: boolean
  /** 随机 UUID，存在 launcher.toml 里；**不含任何硬件信息** */
  machineId: string
  endpoint: string
  stage: string
  errorCode: string
  summary: string
  /** meta 里那几项，摆成人看得懂的行 */
  metaLines: string[]
  /** 出口闸命中的那一处；null = 干净。有值时**不给上传按钮** */
  scanHit: string | null
  /** 设置里「出错时自动上传」现在开着没有 */
  autoOnError: boolean
}

/** 一次上传的结果。 */
export interface LogshipOutcome {
  ok: boolean
  /** 追踪码，形如 `HL-7K3Q9F`。**只有真的拿到了才有值**（红线 1） */
  traceCode: string | null
  truncated: boolean
  /** 给用户看的一整句话。失败时说清楚东西还在哪 */
  message: string
  /** HTTP 状态码；压根没连上时为 null */
  status: number | null
  /** 传不上去时当场导出来的诊断包路径 */
  bundlePath: string | null
  /** 本机日志文件的路径 */
  localLogPath: string
}

// ── 上一次升级没做完（I16 · P1-2） ───────────────────────────────────────

/**
 * 「配置说的是一版、上一次成功的是另一版、而配置那一版的镜像本机还不齐」的现场。
 *
 * 强退会留下它：`.env` 已经写成新版本、镜像还没拉全、回滚没走到。
 * 这时候**不许默默按新配置 `up`** —— 那会去拉一个还没下完的镜像，
 * 用户又看到一次「卡住」。界面上给两个按钮：继续升级 / 回退。
 *
 * **I17 · P0-3 改了两处形状**（客户 2026-09-29 那台 HL-GFV764 就是踩在这里）：
 *
 * * `runningTag` 现在是 `string | null` —— 判据不再要求「必须有容器在跑」。
 *   六个容器全停着（客户那台的现场）时它就是 `null`，卡片照样出。
 * * 新增 `lastGoodTag` —— 「上一次真正成功的版本」（`launcher.toml` 的 `hunter.tag`）。
 *   全停时**回退目标就是它**，所以回退按钮与回退提示一律用
 *   `runningTag ?? lastGoodTag`。
 */
export interface InterruptedUpgrade {
  configTag: string
  /** 正在跑的容器用的版本；**一个都没跑时为 `null`** */
  runningTag: string | null
  /** 上一次真正成功的版本（全停时的回退目标） */
  lastGoodTag: string
  missingImages: string[]
  headline: string
  lines: string[]
}

// ── 升级前置体检（I17 · §4.2②） ─────────────────────────────────────────

/** 「换到哪个源能通」。 */
export interface SwapOffer {
  /** 候选源 id（写进 launcher.toml 的那一个） */
  id: string
  label: string
  prefix: string
}

/**
 * 升级前置体检的结论（**只读**，在改任何配置之前跑）。
 *
 * 0.1.17 的顺序是「先写新 `.env` → 再拉镜像」——**配置先改了，才发现拉不动**。
 * 现在点下「升级」之后先探一次 manifest：当前源有这一版就照原路走（不额外多问），
 * 没有就把「换到哪个源、要下多少」摆给用户，**一个字节的配置都不改**。
 */
export interface Preflight {
  target: string
  currentLabel: string
  currentPrefix: string
  /** 现在这个源上有这一版吗 */
  currentOk: boolean
  /** 当前源没有时，另一个能通的源 */
  offer: SwapOffer | null
  /** 换过去大概要下多少字节；读不到就是 null（不编） */
  offerBytes: number | null
  /** 当前源上没有这一版的原因原话（或者「跳过体检」的说明） */
  reason: string | null
}

// ── 升级进行中的退出拦截（I17 · P0-4） ───────────────────────────────────

/**
 * 「现在退出会留下一个说不清的中间态」那一句话。
 *
 * **只在升级进行中（配置已改写、这一轮还没收尾）出现** —— 常规退出一个提示都没有
 * （U2 的要求，A4-6 专门防这个回归）。界面收到它就弹一个框，两个动作：
 * 「继续退出」与「取消升级并回滚后再退出」。
 */
export interface QuitGuard {
  /** 这一轮正在升到的那一版 */
  targetTag: string
  headline: string
  body: string
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
  /** I16：是用户自己点的「取消」，不是出了错。界面要把这两件事分开说 */
  cancelled: boolean
  message: string
  /**
   * I19 · P0-B：这一轮升级一共用掉几个修复回合。**实测值**（后端计数器给的），
   * 「修不好」那一屏照着它说「自己排查处理了 N 轮」。
   */
  repairRounds?: number
  /** I19：是不是「5 个回合都用完了才停」（区别于守卫判定「不该修」） */
  exhausted?: boolean
  /** I19：失败时的错误码。「把日志交给开发者」按钮原样带给后端 */
  errorCode?: string | null
}

export interface UpgradeStatus {
  running: boolean
  steps: string[]
  result: UpgradeResult | null
  error: string | null
}

// ── 数据备份与恢复（I13 · R6）─────────────────────────────────────────────

/**
 * 这次备份是谁发起的。
 *
 * `missed`（R2）是**启动器自己补跑的那一次**：这台机器上系统定时任务没装上
 * （`schtasks` 三条路全灭），启动器启动满等待时长之后替用户补做一遍。
 * 它**不是** `scheduled` —— 说成「定时备份」就是在编。
 */
export type BackupKind = 'manual' | 'scheduled' | 'missed' | 'pre-upgrade'

export interface BackupFileEntry {
  name: string
  bytes: number
  sha256: string
}

export interface BackupTableRows {
  table: string
  rows: number
}

export interface BackupVolumeEntry {
  short: string
  file: string
  bytes: number | null
  error: string | null
}

/**
 * 一次备份。
 *
 * `dumpBytes` 为 null 说明数据库那一半没做成，原因在 `dumpError`；
 * `sqlBytes` / `sqlError` 是 0.1.12 及之前那种纯 SQL 备份留下的老字段
 * （那些备份照样列得出来、恢复得了）。
 */
export interface BackupMeta {
  id: string
  tag: string
  at: string
  kind: BackupKind
  launcherVersion: string
  project: string
  entries: BackupFileEntry[]
  dumpBytes: number | null
  dumpError: string | null
  /** `pg_restore --list` 读得出目录吗。**读不出就不算成功的备份** */
  verified: boolean
  verifyNote: string
  tableCount: number | null
  tables: BackupTableRows[]
  rowsTotal: number | null
  tablesNote: string
  volumes: BackupVolumeEntry[]
  totalBytes: number
  elapsedMs: number
  dir: string
  files: string[]
  sqlBytes: number | null
  sqlError: string | null
}

export interface BackupSettings {
  enabled: boolean
  time: string
  dir: string
  effectiveDir: string
  keepDays: number
  includeSessions: boolean
  includeSkills: boolean
  lastOkAt: string
  lastError: string
  lastRunAt: string
  failStreak: number
  count: number
  totalBytes: number
  diskFreeBytes: number | null
  suggestedDir: string
  externalSuggestions: string[]
  /**
   * 上一次挂定时任务时系统报的原话（I16 · P0-3）。空 = 上一次挂成了。
   *
   * 非空时运行面板出一条红横幅：客户那台 Windows 上
   * `schtasks /Create` 每次都失败，0.1.15 只写进日志，
   * 于是他以为自己有每日自动备份 —— 实际上一次都没跑过。
   */
  scheduleError: string

  // ── R2 · B 层兜底（方案 §4.3）────────────────────────────────────────────
  /** 关掉它 = 回到 R2 之前：Windows 上任务装不上就一次都不跑 */
  windowsFallback: boolean
  /** **启动后等多久才允许补跑**（分钟）。默认 120，范围 5–720 */
  fallbackDelayMins: number
  /** 两次成功备份之间最长多久（小时）。本轮不给用户改 */
  fallbackIntervalHours: number
  /**
   * 本机自动备份现在靠哪条路（运行面板那行常驻状态用它）：
   * `scheduler` = 系统定时任务（**错过会补跑**）；
   * `fallback` = 启动器自己补跑（**错过了不补**）；
   * `off` = 关着。
   *
   * 两种机制的能力不一样，界面上必须分开说，不能混成一句「已开启」。
   */
  autoMode: 'scheduler' | 'fallback' | 'off'
}

export interface ScheduleStatus {
  mech: string
  supported: boolean
  installed: boolean
  enabled: boolean
  wanted: boolean
  time: string
  path: string
  nextRun: string
  lines: string[]
  reason: string
}

export interface RestorePreflight {
  id: string
  dir: string
  tag: string
  currentTag: string
  needsUpgrade: boolean
  hasDump: boolean
  legacySql: boolean
  verified: boolean
  checksumMismatch: string[]
  volumes: string[]
  blocked: string | null
  lines: string[]
}

export interface RestoreReport {
  from: string
  safetyBackup: string | null
  steps: string[]
  restoredVolumes: string[]
  jwtRestored: boolean
  ready: number
  total: number
  elapsedMs: number
  headline: string
}

// ── 删除应用（I13 · R4）───────────────────────────────────────────────────

export type UninstallScope = 'app-only' | 'app-and-data'

export interface UninstallImageItem {
  reference: string
  bytes: number | null
}

export interface UninstallPlan {
  scope: UninstallScope
  confirmPhrase: string
  containers: string[]
  volumes: VolumeInfo[]
  volumesBytes: number | null
  images: UninstallImageItem[]
  imagesBytes: number | null
  imagesFound: number
  homeBytes: number
  runtimeBytes: number | null
  builtinRuntime: boolean
  /** 「保留数据的同时删运行环境」为什么不行。为 null 才允许勾 */
  runtimeBlocked: string | null
  tableCount: number | null
  lastWrite: string | null
  backups: number
  lastBackupAt: string | null
  backupDir: string
  scheduleInstalled: boolean
  estFreedBytes: number
  warnings: string[]
  lines: string[]
}

export interface UninstallOptions {
  scope: UninstallScope
  removeImages: boolean
  removeRuntime: boolean
  backupFirst: boolean
  confirm: string
}

export interface UninstallReport {
  scope: string
  steps: string[]
  removedContainers: number
  removedVolumes: string[]
  removedImages: string[]
  removedRuntime: boolean
  removedSchedule: boolean
  kept: string[]
  backupId: string | null
  freedBytes: number
  elapsedMs: number
  headline: string
  failures: string[]
}

// ── 异常监测与一键清理（I13 · R7）─────────────────────────────────────────

export interface MonitorAlert {
  id: string
  level: MonitorLevel
  title: string
  detail: string
  facts: string[]
  actions: string[]
  advice: string[]
  needsAi: boolean
}

export interface MonitorAlerts {
  at: string
  alerts: MonitorAlert[]
  notify: string[]
  reasons: Record<string, string>
}

export interface CleanupOldImage {
  reference: string
  id: string
  bytes: number | null
}

export interface CleanupOrphanVolume {
  name: string
  short: string
  bytes: number | null
}

export interface CleanupPlan {
  oldImages: CleanupOldImage[]
  oldImagesBytes: number
  orphanVolumes: CleanupOrphanVolume[]
  orphanVolumesBytes: number
  expiredBackups: string[]
  expiredBackupsBytes: number
  totalBytes: number
  lines: string[]
  reasons: string[]
}

export interface CleanupDone {
  images: string[]
  volumes: string[]
  backups: string[]
  freedBytes: number
  steps: string[]
  failures: string[]
  headline: string
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
  /**
   * I19 · B.4：这一步是**规则直接判的**还是**问了模型**。
   * 规则判的更快更准、也不花 token —— 过程流里要能分辨（对用户是诚实）。
   * 只有「找到原因」那一类事件会带上它。
   */
  by?: 'rule' | 'model' | 'orchestrator' | 'user'
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

/**
 * 复查的六种结论。和 Rust 侧 `selfcheck::Posture` 一一对应。
 *
 * `stopped` 是 I16 补的：六个容器都建齐了、但一个在跑的都没有。
 * 在这之前它被并进 `partial`，界面于是把「容器退出了」说成「还没就绪」。
 *
 * `runtime-down` 是 R1 补的：**磁盘上装过，但运行时问不出来**（内置虚拟机 /
 * Docker Desktop 当时没在跑）。在这之前它被并进 `absent`，于是「装过、只是没起来」
 * 的机器会被当成从没装过 —— 再走一遍安装流程、重新填一次 key。
 * **它绝不能退化成 `absent`**：`absent` 的下游是完整安装，而这一档一个字节都不该动。
 */
export type Posture =
  | 'absent'
  | 'incomplete'
  | 'stopped'
  | 'runtime-down'
  | 'partial'
  | 'healthy'

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
