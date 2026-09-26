/*
 * 演示数据（总控规则红线 1）
 * ---------------------------------------------------------------------------
 * 只在 VITE_DEMO=1 的开发构建里被引用，用途是：浏览器/Xvfb 里预览界面、与三张视觉稿做截图比对。
 * 界面右上角会常驻一个「演示数据」角标，发布构建拿不到这个模块的数据（见 vite.config.ts 的构建期断言）。
 *
 * 取值原则（与 M0 预研结论一致，待办池 P2-1 已记）：
 *   · 布局相关的比例照视觉稿（例如 opencode 58% / api 52% 两条进度条的长度）；
 *   · **数值一律用 M0 实测值**，不抄视觉稿上那些已被证伪的数字：
 *       key 前缀 hunt_tools_（不是 hk_）、模型别名 hunter-chat（不是 hunter-default）、
 *       镜像源 GHCR（不是阿里云杭州）、镜像合计 849 MB = 810.6 MiB（不是 2.1 GB）、
 *       端口 3100/8100/3921/5442/6479（不是 3100/8000/3901/5432/6379）。
 *   · 运行面板的「今日对话」「晨报」两张卡在**演示模式下也显示「—」**：
 *     M0 §3.6 实测上游没有这两个数字的接口，做一个能显示数字的演示等于承诺了一个不存在的能力。
 */

import type {
  AssistAutoSnapshot,
  AppInfo,
  BackupMeta,
  BackupSettings,
  BootState,
  BuiltinRuntimeStatus,
  DataCheck,
  DiagSection,
  HostMetrics,
  AssistState,
  DockerInfo,
  ImagePull,
  KeyCheckResult,
  KeptKey,
  LauncherSettings,
  LauncherUpdate,
  MissingEndpoint,
  OfflineImport,
  OneClickFeedback,
  LogshipPreview,
  LogshipOutcome,
  InterruptedUpgrade,
  OwnKeyCheck,
  PullProgress,
  RegistryProbe,
  RuntimeMetrics,
  RuntimeStatus,
  SelfCheckReview,
  ServiceMetrics,
  ServiceStatus,
  StackOpResult,
  StackPlan,
  StorageMetrics,
  TakeoverCandidate,
  TakeoverState,
  TelemetryView,
  UpgradeCheck,
  CleanupPlan,
  MonitorAlerts,
  RestorePreflight,
  ScheduleStatus,
  UninstallPlan,
} from './types'

/** 构建产物里能被 grep 到的标记，CI 用它确认发布包里没有演示数据。 */
export const DEMO_MARKER = 'HUNTER_DEMO_DATA_MARKER'

/**
 * 演示数据里的版本号。**发版时要跟着 `package.json` 一起改** ——
 * I8 之前它一直停在 `0.1.0`，于是每一轮的截图右上角都写着「启动器 v0.1.0」，
 * 看图的人分不清那是哪一版的界面（I8 截图时才发现）。
 */
export const DEMO_LAUNCHER_VERSION = '0.1.16'
export const DEMO_HUNTER_TAG = '1.1.0'
export const DEMO_LATEST_TAG = '1.2.0'
export const DEMO_REGISTRY = 'ghcr.io/agentpit-io'
export const DEMO_REGISTRY_LABEL = 'GHCR'

/** 编造的假 key，形状与真 key 一致（前缀 + 32 位），但不是任何一把真实签发的 key。 */
export const DEMO_KEY = 'hunt_tools_9f2c7a41xxxxxxxxxxxxxxxxxxxxb3e1'

export const demoAppInfo: AppInfo = {
  launcherVersion: DEMO_LAUNCHER_VERSION,
  platform: 'linux',
  arch: 'x86_64',
  windowChrome: 'custom',
}

export const demoDocker: DockerInfo = {
  installed: true,
  daemonRunning: true,
  runtime: 'docker-engine',
  runtimeLabel: 'Docker Engine 29.8.1',
  clientVersion: '29.8.1',
  serverVersion: '29.8.1',
  composeVersion: 'v5.5.1',
  arch: 'x86_64',
  dockerPath: '/usr/bin/docker',
  dockerPathSource: 'PATH',
  dockerLinkTarget: null,
  dockerProbe: [{ source: 'PATH', candidate: '/usr/bin/docker', outcome: 'found' }],
  composeMode: 'docker compose 插件 v5.5.1',
  wsl: null,
  meetsMinimum: true,
  problem: null,
  installGuide: null,
}

/** 「没装 Docker」那一页的演示态（截图脚本用 HUNTER_DEMO_PAGE=docker-missing 打开）。 */
export const demoDockerMissing: DockerInfo = {
  installed: false,
  daemonRunning: false,
  runtime: 'unknown',
  runtimeLabel: null,
  clientVersion: null,
  serverVersion: null,
  composeVersion: null,
  arch: 'x86_64',
  dockerPath: null,
  dockerPathSource: null,
  dockerLinkTarget: null,
  // 演示的是 I4 之后的样子：探过哪些位置、分别什么结果，一条不落地列出来
  dockerProbe: [
    { source: 'PATH', candidate: '/usr/bin/docker', outcome: 'missing' },
    { source: '已知位置', candidate: '/usr/local/bin/docker', outcome: 'missing' },
    { source: '已知位置', candidate: '/snap/bin/docker', outcome: 'missing' },
  ],
  composeMode: null,
  wsl: null,
  meetsMinimum: false,
  problem: '按顺序探了 3 个位置，都没有可执行的 docker。\n装在别处的话，可以在 ~/.hunter/launcher.toml 的 [runtime] 段里写 docker_path 指给它。',
  installGuide: {
    platform: 'linux',
    title: '在 Linux 上装 Docker',
    steps: [
      { text: '官方便利脚本：curl -fsSL https://get.docker.com | sh（国内可以加 --mirror Aliyun）。', url: 'https://docs.docker.com/engine/install/' },
      { text: '装完把自己加进 docker 组，免得每条命令都要 sudo：sudo usermod -aG docker $USER，然后**重新登录**一次。', url: null },
      { text: '确认 compose 插件也在：docker compose version，要 v2.20 及以上。', url: 'https://docs.docker.com/compose/install/linux/' },
      { text: '服务器没有桌面环境的话，用 hunter-launcher --headless 走纯命令行安装，流程和界面版完全一样。', url: null },
    ],
    licenseNote:
      '提示：Docker Desktop 对「员工 250 人以上或年收入超过 1000 万美元」的企业是收费的。个人与小团队免费。不想用它的话，mac 上可以换 OrbStack，Linux 上直接用 Docker Engine（本来就免费），Podman 也能跑但属于实验支持。',
  },
}

export const demoKeyCheck: KeyCheckResult = {
  valid: true,
  reason: 'ok',
  quota: {
    usedToday: 115,
    limitDaily: 300000,
    remaining: 299885,
    exhausted: false,
    resetAt: '2026-09-20T00:00:00+08:00',
    rpm: 20,
    concurrency: 4,
  },
  models: [
    { id: 'hunter-chat', purpose: '默认对话与工具调用 · 快、便宜' },
    { id: 'hunter-deep', purpose: '深度分析与长任务 · 更强但更贵' },
  ],
  message: null,
  code: null,
}

/**
 * 演示模式下「上次保留的 key」。
 *
 * **默认当成没有** —— `key` 那一页的演示要看的是完整的输入流程。
 * 想看「沿用上次保留的 key」那一档（I14 · F3）用 `__HUNTER_DEMO_PAGE__ = 'key-kept'`，
 * 那正是「只删除应用，保留数据」之后重新安装时的样子；
 * `'key-shape'` 是它旁边那一档 —— 留着的东西不是一把 key 的样子，沿用不了但要说一句。
 */
export function demoKeptKey(page: string | null): KeptKey {
  // `.env` 里留着东西、但它不是一把 hunter key 的样子（I14 · F3 收尾补的那一档）
  if (page === 'key-shape') {
    return {
      present: false,
      masked: 'hunt_tools_****',
      path: '<用户目录>/.hunter/app/.env',
      check: null,
      badShape: true,
    }
  }
  if (page !== 'key-kept') {
    return {
      present: false,
      masked: '',
      path: '<用户目录>/.hunter/app/.env',
      check: null,
      badShape: false,
    }
  }
  return {
    present: true,
    masked: 'hunt_tools_****QMo2',
    path: '<用户目录>/.hunter/app/.env',
    check: demoKeyCheck,
    badShape: false,
  }
}

export const demoBootState: BootState = {
  installed: false,
  running: false,
  locale: 'zh-CN',
  hunterTag: '1.2.0',
  workDir: '~/.hunter',
  route: 'welcome',
  posture: 'absent',
  headline: '这台机器上还没有装过 Hunter',
  adopted: false,
  launcherVersion: DEMO_LAUNCHER_VERSION,
  installedAt: '',
  lastHealthyAt: '',
  volumeCount: 0,
  elapsedMs: 264,
  interruptedUpgrade: null,
}

export const demoOwnKeyCheck: OwnKeyCheck = {
  ok: true,
  via: 'gateway',
  message: '用 Hunter 内置额度网关 · 模型 hunter-chat',
  schemaSanitize: false,
  models: ['hunter-chat', 'hunter-deep'],
}

/** 镜像源测速的演示值。真实探测在 Rust 侧，这里只是让截图有内容。 */
export const demoProbes: RegistryProbe[] = [
  { id: 'ghcr', label: 'GHCR · GitHub', prefix: DEMO_REGISTRY, available: true, elapsedMs: 1993, detail: null },
  { id: 'tencent', label: '腾讯云 · 香港', prefix: 'hkccr.ccs.tencentyun.com/agentpit', available: true, elapsedMs: 10765, detail: null },
]

/** 六个镜像的大小取自 M0 §4.1（linux/amd64 压缩后字节数）。 */
const IMAGE_SIZES: { service: string; shortRef: string; tag: string; bytes: number }[] = [
  { service: 'opencode', shortRef: 'hunter-community-opencode', tag: '1.2.0', bytes: 152979569 },
  { service: 'api', shortRef: 'hunter-community-api', tag: '1.2.0', bytes: 214680985 },
  { service: 'web', shortRef: 'hunter-community-web', tag: '1.2.0', bytes: 331344498 },
  { service: 'llm-shim', shortRef: 'hunter-community-llm-shim', tag: '1.2.0', bytes: 18054459 },
  { service: 'postgres', shortRef: 'postgres', tag: '16-alpine', bytes: 115971686 },
  { service: 'redis', shortRef: 'redis', tag: '7-alpine', bytes: 16148070 },
]

/** 每个镜像的完成度，照视觉稿第 2 张的进度条长度（前两条在下载，后四条已完成）。 */
const IMAGE_PROGRESS: Record<string, number> = {
  opencode: 0.58,
  api: 0.52,
  web: 1,
  'llm-shim': 1,
  postgres: 1,
  redis: 1,
}

export const demoImages: ImagePull[] = IMAGE_SIZES.map((it) => {
  const ratio = IMAGE_PROGRESS[it.service] ?? 0
  const base = it.shortRef.startsWith('hunter-') ? DEMO_REGISTRY : 'docker.io/library'
  return {
    service: it.service,
    ref: `${base}/${it.shortRef}:${it.tag}`,
    shortRef: it.shortRef,
    tag: it.tag,
    totalBytes: it.bytes,
    downloadedBytes: Math.round(it.bytes * ratio),
    state: ratio >= 1 ? 'done' : 'downloading',
    sizeUnknown: false,
    seconds: ratio >= 1 ? Math.round(it.bytes / 4_800_000) : null,
  }
})

const demoDone = demoImages.reduce((a, i) => a + i.downloadedBytes, 0)
const demoTotal = demoImages.reduce((a, i) => a + i.totalBytes, 0)

export const demoPull: PullProgress = {
  phase: 'pulling',
  registry: DEMO_REGISTRY,
  registryLabel: DEMO_REGISTRY_LABEL,
  images: demoImages,
  totalBytes: demoTotal,
  downloadedBytes: demoDone,
  percent: Math.round((demoDone / demoTotal) * 100),
  speedBps: 4_800_000,
  etaSeconds: 80,
  attempt: 1,
  log: [
    '17:42:08 api layer 4c1efbb5d2e0 Pull complete',
    '17:42:07 opencode layer a9f2c31b7ad4 Pull complete',
  ],
  error: null,
  errorCode: null,
}

export const demoRuntime: RuntimeStatus = {
  hunterTag: DEMO_HUNTER_TAG,
  quota: {
    usedToday: 42300,
    limitDaily: 300000,
    remaining: 257700,
    exhausted: false,
    resetAt: '2026-09-20T00:00:00+08:00',
    rpm: 20,
    concurrency: 4,
  },
  latestTag: DEMO_LATEST_TAG,
  running: true,
  uptimeSeconds: 3 * 86400 + 6 * 3600,
  webUrl: 'http://localhost:3100',
  // 免费版只允许本机访问（I7），演示数据也照这个来
  webLanExposed: false,
  services: [
    { service: 'web', state: 'running', health: 'healthy', port: 3100, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-web:${DEMO_HUNTER_TAG}` },
    { service: 'api', state: 'running', health: 'healthy', port: 8100, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-api:${DEMO_HUNTER_TAG}` },
    { service: 'opencode', state: 'running', health: 'healthy', port: 3921, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-opencode:${DEMO_HUNTER_TAG}` },
    { service: 'llm-shim', state: 'running', health: 'healthy', port: null, bind: null, exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-llm-shim:${DEMO_HUNTER_TAG}` },
    { service: 'postgres', state: 'running', health: 'healthy', port: 5442, bind: '127.0.0.1', exitCode: null , image: `docker.io/library/postgres:16-alpine` },
    { service: 'redis', state: 'running', health: 'healthy', port: 6479, bind: '127.0.0.1', exitCode: null , image: `docker.io/library/redis:7-alpine` },
  ],
  upstream: {
    reachable: true,
    apiKeyConfigured: true,
    llmSource: 'env',
    llmModel: 'hunter-chat',
    builtinQuota: true,
    dataSupplyConfigured: true,
    reason: null,
  },
  dataSource: 'Hunter 网关',
  dataSourceSub: '平台数据供给已配置 · 自有数据源个数需登录后才能读',
  // I14 · F4：装完那一步的结论（演示里显示「挂上了」的样子）
  postInstallNotes: ['已按你的备份设置把每天 00:00 的自动备份重新挂上（macOS 用户级 LaunchAgent）'],
  missing: [
    { id: 'conversations', endpoint: 'GET /api/system/metrics/daily（今日对话数、深度分析次数）' },
    { id: 'morning_brief', endpoint: 'GET /api/system/metrics/daily（晨报开关与推送时间）' },
    { id: 'user_sources', endpoint: 'GET /api/user_sources 的免登录只读计数' },
    { id: 'last_tool_call', endpoint: 'GET /api/system/last-tool-call' },
  ],
  env: [
    { key: 'model', value: '网关 · hunter-chat' },
    { key: 'docker', value: 'Docker Engine 29.8.1 · x86_64' },
    { key: 'registry', value: DEMO_REGISTRY_LABEL },
    { key: 'autostart', value: 'off' },
    { key: 'telemetry', value: 'off' },
  ],
  log: [
    '08:30:02 morning_brief sent to wecom 持仓 12 只 · 2 条异动',
    '08:29:41 uzi_scan_trap 600519 ok 1.8s',
    '08:29:39 uzi_quote 300750 ok 0.6s',
    '08:29:38 kronos_forecast 601899 ok 3.2s',
    '00:00:00 quota reset 300,000 tokens',
  ],
  apiKeyConfigured: true,
  installed: true,
}

export const demoStartingServices: RuntimeStatus['services'] = [
  { service: 'postgres', state: 'running', health: 'healthy', port: 5442, bind: '127.0.0.1', exitCode: null , image: `docker.io/library/postgres:16-alpine` },
  { service: 'redis', state: 'running', health: 'healthy', port: 6479, bind: '127.0.0.1', exitCode: null , image: `docker.io/library/redis:7-alpine` },
  { service: 'llm-shim', state: 'running', health: 'healthy', port: null, bind: null, exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-llm-shim:${DEMO_HUNTER_TAG}` },
  { service: 'api', state: 'running', health: 'healthy', port: 8100, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-api:${DEMO_HUNTER_TAG}` },
  { service: 'opencode', state: 'running', health: 'starting', port: 3921, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-opencode:${DEMO_HUNTER_TAG}` },
  // 还没创建出来的容器 docker 报不出绑定地址 —— 那就是 null，不猜（红线 1）
  { service: 'web', state: 'created', health: 'pending', port: 3100, bind: null, exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-web:${DEMO_HUNTER_TAG}` },
]

export const demoSettings: LauncherSettings = {
  locale: 'zh-CN',
  autostart: false,
  checkUpdate: true,
  // 免费版只允许本机访问，所以这一项恒为 false（演示模式也不该显示一个不存在的状态）
  webLanExposed: false,
  registry: 'ghcr',
  hunterTag: DEMO_HUNTER_TAG,
  workDir: '~/.hunter',
  telemetry: false,
  // 空字符串 = 暂未开启上报。演示模式下也**不能**编一个端点出来 ——
  // 那等于在界面上承诺一个不存在的服务（红线 1）。
  telemetryEndpoint: '',
  modelMode: 'gateway',
  modelBaseUrl: 'https://hunter.agentpit.io/api/saas/llm/v1',
  modelName: 'hunter-chat',
  registryPrefix: DEMO_REGISTRY,
  // AI 诊断助手默认开（I4）
  assist: true,
  // I5：授权档位。演示里显示「已授权自动驾驶」的样子
  assistMode: 'auto',
  assistConsentedAt: '2026-09-21 10:30:00',
  dockerPath: '/usr/bin/docker',
  // I7：授权页上那一项勾，默认勾着
  allowInstallRuntime: true,
  installRoute: 'builtin',
  builtinRuntimeInstalled: false,
  builtinRuntimeRunning: false,
  takeoverProject: '',
  autoUploadLogs: false,
  lastTraceCode: '',
  lastUploadAt: '',
}

/** 内置运行时的演示态：**没装**（默认就是没装，别编一个装好的样子出来）。 */
export const demoBuiltinRuntime: BuiltinRuntimeStatus = {
  supported: false,
  unsupportedReason:
    '内置运行时目前只做了 macOS 这一条路线（演示数据；真机上这一行来自 Rust 的真实判断）。',
  installed: false,
  running: false,
  profile: 'hunter',
  dir: '~/.hunter/runtime',
  socket: '~/.hunter/runtime/colima/hunter/docker.sock',
  items: [],
  installedAt: '',
  downloadBytes: 0,
}

/** 「本机已经有一套 Hunter」的演示数据（截图用）。 */
export const demoTakeoverCandidates: TakeoverCandidate[] = [
  {
    project: 'hunter-community',
    containers: [
      'hunter-community-web-1',
      'hunter-community-api-1',
      'hunter-community-opencode-1',
      'hunter-community-postgres-1',
      'hunter-community-redis-1',
    ],
    ports: [3100, 8100, 3921, 5442, 6479],
    workingDir: '~/hunter-community',
    configFiles: '~/hunter-community/docker-compose.yml',
    webPort: 3100,
  },
]

export const demoTakeoverState: TakeoverState = {
  active: true,
  manageable: true,
  project: 'hunter-community',
  workingDir: '~/hunter-community',
  since: '2026-09-21 18:20:00',
  webUrl: 'http://localhost:3100',
  containers: [
    { name: 'hunter-community-web-1', status: 'Up 2 days', image: 'hunter-community-web:1.2.0', ports: '0.0.0.0:3100->3000/tcp' },
    { name: 'hunter-community-api-1', status: 'Up 2 days', image: 'hunter-community-api:1.2.0', ports: '0.0.0.0:8100->8000/tcp' },
    { name: 'hunter-community-opencode-1', status: 'Up 2 days', image: 'hunter-community-opencode:1.2.0', ports: '0.0.0.0:3921->3921/tcp' },
    { name: 'hunter-community-postgres-1', status: 'Up 2 days', image: 'postgres:16-alpine', ports: '0.0.0.0:5442->5432/tcp' },
    { name: 'hunter-community-redis-1', status: 'Up 2 days', image: 'redis:7-alpine', ports: '0.0.0.0:6479->6379/tcp' },
  ],
  note: '',
}

/** 一键反馈的演示态。**scanHit 是 null（干净），bundlePath 用 ~ 开头的脱敏路径。** */
export const demoOneClick: OneClickFeedback = {
  bundlePath: '~/.hunter/diagnostics/hunter-diagnostics-20260921-183045.zip',
  bundleBytes: 24_576,
  issueUrl: 'https://github.com/agentpit-io/HunterLauncher/issues/new?title=%E6%BC%94%E7%A4%BA&body=%E6%BC%94%E7%A4%BA',
  issueTitle: '[部署问题] E_PULL_FAILED AI 自动安装没能把问题解决',
  issueBody: '## 遇到了什么\n\n演示数据。\n\n## 环境\n\n```\n启动器: 0.1.7\n系统: macos aarch64\n```\n',
  scanHit: null,
  note: '演示数据：诊断包只在你自己的机器上，启动器没有把它发给任何人。',
}

/** 遥测队列的演示内容。开关关着时真实队列是空的，这里演示的是「开了之后长什么样」。 */
export const demoTelemetry: TelemetryView = {
  enabled: false,
  endpoint: '',
  queuePath: '~/.hunter/telemetry/queue.jsonl',
  lines: [
    '{"ts":"2026-09-20 09:12:03","install_id":"11111111-2222-4333-8444-555555555555","event":"launcher_start","version":"0.1.0","os":"linux","arch":"x86_64","locale":"zh-CN"}',
    '{"ts":"2026-09-20 09:12:04","install_id":"11111111-2222-4333-8444-555555555555","event":"docker_detected","runtime":"DockerEngine","version":"29.8.1","ok":true}',
  ],
  events: [
    'launcher_start',
    'docker_detected',
    'setup_step',
    'pull_done',
    'hunter_started',
    'error',
    'update',
  ],
}

export const demoMissing: MissingEndpoint[] = demoRuntime.missing

/** 反馈页的诊断分节演示。真实内容来自 Rust 侧的 feedback::collect。 */
export const demoDiagSections: DiagSection[] = [
  {
    id: 'summary',
    title: '概要',
    body:
      '时间: 2026-09-20 09:12:03\n启动器: 0.1.0\n系统: linux x86_64\n' +
      'Docker: DockerEngine · server 29.8.1 · compose v5.5.1\n' +
      'Hunter tag: 1.2.0\n镜像源: ghcr (ghcr.io/agentpit-io)\n' +
      '端口: web 3101 · api 8101 · opencode 3922 · postgres 5443 · redis 6480\n' +
      '模型: gateway · hunter-chat\n遥测: 关 · 上报端点: （无 · 暂未开启上报）\n',
    defaultOn: true,
    note: null,
  },
  {
    id: 'ps',
    title: 'docker compose ps',
    body:
      'web        running    健康     端口 3101\napi        running    健康     端口 8101\n' +
      'opencode   running    健康     端口 3922\nllm-shim   running    健康     端口 内部\n' +
      'postgres   running    健康     端口 5443\nredis      running    健康     端口 6480\n',
    defaultOn: true,
    note: null,
  },
  {
    id: 'env',
    title: '.env（只保留白名单四项，其余的值一律移除）',
    body:
      'DATA_SOURCE_PROVIDER=hunter\nHUNTER_API_KEY=<已移除>\nJWT_SECRET=<已移除>\n' +
      'LLM_BASE_URL=hunter.agentpit.io\nLLM_DEFAULT_MODEL=hunter-chat\nPOSTGRES_PASSWORD=<已移除>\n',
    defaultOn: true,
    note: null,
  },
  {
    id: 'last-tool-call',
    title: '最近一次工具调用摘要',
    body: '',
    defaultOn: false,
    note: '上游 api 没有 /api/system/last-tool-call（方案 §13 列了，实测不存在）。',
  },
]

export const demoLauncherLog: string[] = [
  '2026-09-19 17:45:12  INFO   launcher  状态 Starting → Done',
  '2026-09-19 17:45:12  INFO   compose   6/6 服务健康，用时 42.3s',
  '2026-09-19 17:44:30  INFO   compose   docker compose -p hunter up -d',
  '2026-09-19 17:44:29  INFO   config    写入 ~/.hunter/app/.env（权限 600）',
  '2026-09-19 17:44:29  INFO   config    端口 3100 被占用，web 改用 3101',
  '2026-09-19 17:42:51  INFO   compose   pull 完成，849 MB / 6 个镜像 / 用时 3m21s',
  '2026-09-19 17:39:30  INFO   registry  使用镜像源 ghcr.io/agentpit-io',
  '2026-09-19 17:39:29  INFO   gateway   key 校验通过，今日剩余 299,885 tokens',
  '2026-09-19 17:39:25  INFO   docker    Docker Engine 29.8.1 · Compose v5.5.1',
  '2026-09-19 17:39:24  INFO   launcher  Hunter 启动器 0.1.0 启动（linux/x86_64）',
]

/** 日志页「容器日志」那一档的演示内容。 */
export const demoComposeLog: string[] = [
  'hunter-api-1       | INFO:     127.0.0.1:52344 - "GET /api/health HTTP/1.1" 200 OK',
  'hunter-web-1       | ready - started server on 0.0.0.0:3000',
  'hunter-opencode-1  | opencode server listening on 127.0.0.1:3901',
  'hunter-llm-shim-1  | shim ready, upstream=https://hunter.agentpit.io/api/saas/llm/v1',
  'hunter-postgres-1  | database system is ready to accept connections',
  'hunter-redis-1     | Ready to accept connections tcp',
]


// ── M4 · 更新 / 备份 / 离线包的演示数据 ──────────────────────────────────
//
// 同样按红线 1 的原则取值：版本号、错误码、路径都用真实形状，
// 不编造一个"看起来很厉害"的场景。

export const demoLauncherUpdate: LauncherUpdate = {
  available: true,
  current: DEMO_LAUNCHER_VERSION,
  version: '0.1.10',
  notes: '修了拉取页在慢网络下 ETA 抖动的问题；升级失败回滚后不再重复提示。',
  date: '2026-09-21T02:00:00Z',
  // 演示的是 .deb 这一路 —— I8 起它是「弹系统授权框再自己装」那一支，
  // 界面上要多显示一段「为什么会弹密码框」，预览时更有用
  canSelfInstall: false,
  installKind: 'deb',
  reason: null,
}

export const demoHunterUpdate: UpgradeCheck = {
  current: DEMO_HUNTER_TAG,
  latest: DEMO_LATEST_TAG,
  hasUpdate: true,
  notes:
    '## 1.2.0\n\n- 晨报支持企业微信群机器人\n- opencode 升到 0.5.x，工具调用更稳\n- 修复 postgres 16 下的迁移顺序问题\n\n升级须知：本版本会跑一次数据库迁移，升级前请确认备份已完成。',
  notesUrl: 'https://github.com/agentpit-io/hunter-community/releases/tag/v1.2.0',
  publishedAt: '2026-09-19T09:47:29Z',
  majorJump: false,
  reason: null,
}

const demoFiles = ['.env', 'docker-compose.yml', 'docker-compose.launcher.yml', 'launcher.toml']

function demoBackup(id: string, at: string, kind: BackupMeta['kind']): BackupMeta {
  return {
    id,
    tag: DEMO_HUNTER_TAG,
    at,
    kind,
    launcherVersion: '0.1.13',
    project: 'hunter',
    entries: [
      { name: 'hunter.dump', bytes: 486_233, sha256: '3f2a…' },
      { name: 'secrets.tar.gz', bytes: 1_204, sha256: '9b71…' },
    ],
    dumpBytes: 486_233,
    dumpError: null,
    verified: true,
    verifyNote: 'pg_restore --list 读出 214 条目录项',
    tableCount: 23,
    tables: [
      { table: 'user_preference', rows: 2 },
      { table: 'schema_migrations', rows: 23 },
      { table: 'users', rows: 2 },
    ],
    rowsTotal: 31,
    tablesNote: '',
    volumes: [
      { short: 'hunter_secrets', file: 'secrets.tar.gz', bytes: 1_204, error: null },
      { short: 'hunter_user_skills', file: 'user_skills.tar.gz', bytes: 8_912, error: null },
      { short: 'hunter_opencode_data', file: 'opencode_data.tar.gz', bytes: 142_336, error: null },
    ],
    totalBytes: 654_812,
    elapsedMs: 9_120,
    dir: `/home/user/Hunter-backups/${id}`,
    files: demoFiles,
    sqlBytes: null,
    sqlError: null,
  }
}

export const demoBackups: BackupMeta[] = [
  demoBackup('hunter-20260923-0000-v1.2.0', '2026-09-23 00:00:03', 'scheduled'),
  demoBackup('hunter-20260922-0000-v1.2.0', '2026-09-22 00:00:02', 'scheduled'),
  demoBackup('hunter-20260921-1004-v1.2.0', '2026-09-21 10:04:31', 'pre-upgrade'),
]

export const demoBackupSettings: BackupSettings = {
  enabled: true,
  time: '00:00',
  dir: '',
  effectiveDir: '/home/user/Hunter-backups',
  keepDays: 3,
  includeSessions: true,
  includeSkills: true,
  lastOkAt: '2026-09-23 00:00:03',
  lastError: '',
  lastRunAt: '2026-09-23 00:00:03',
  failStreak: 0,
  count: 3,
  totalBytes: 1_964_436,
  diskFreeBytes: 76_836_319_232,
  suggestedDir: '/home/user/Hunter-backups',
  externalSuggestions: ['/media/user/T7/Hunter 备份'],
  scheduleError: '',
}

export const demoSchedule: ScheduleStatus = {
  mech: 'systemd --user 定时器',
  supported: true,
  installed: true,
  enabled: true,
  wanted: true,
  time: '00:00',
  path: '<用户目录>/.config/systemd/user/hunter-backup.timer',
  nextRun: 'Thu 2026-09-24 00:00:00 CST  14h left  -  -  hunter-backup.timer',
  lines: ['systemctl --user is-enabled：enabled'],
  reason: '',
}

export const demoPreflight: RestorePreflight = {
  id: 'hunter-20260923-0000-v1.2.0',
  dir: '<用户目录>/Hunter-backups/hunter-20260923-0000-v1.2.0',
  tag: DEMO_HUNTER_TAG,
  currentTag: DEMO_HUNTER_TAG,
  needsUpgrade: false,
  hasDump: true,
  legacySql: false,
  verified: true,
  checksumMismatch: [],
  volumes: ['hunter_secrets', 'hunter_user_skills', 'hunter_opencode_data'],
  blocked: null,
  lines: ['逐个核过 6 个文件的 sha256', '备份里有这些数据卷：hunter_secrets、hunter_user_skills、hunter_opencode_data'],
}

export function demoUninstallPlan(scope: string): UninstallPlan {
  const all = scope === 'app-and-data'
  return {
    scope: all ? 'app-and-data' : 'app-only',
    confirmPhrase: all ? '删除应用和数据' : '删除应用',
    containers: ['hunter-web-1', 'hunter-api-1', 'hunter-opencode-1', 'hunter-llm-shim-1', 'hunter-postgres-1', 'hunter-redis-1'],
    volumes: [
      { name: 'hunter_hunter_pg_data', short: 'hunter_pg_data', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_pg_data/_data', sizeBytes: 50_800_000 },
      { name: 'hunter_hunter_secrets', short: 'hunter_secrets', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_secrets/_data', sizeBytes: 4_096 },
      { name: 'hunter_hunter_opencode_data', short: 'hunter_opencode_data', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_opencode_data/_data', sizeBytes: 380_000 },
    ],
    volumesBytes: 51_300_000,
    images: [
      { reference: 'ghcr.io/agentpit-io/hunter-community-web:1.2.0', bytes: 121_000_000 },
      { reference: 'ghcr.io/agentpit-io/hunter-community-api:1.2.0', bytes: 1_900_000_000 },
    ],
    imagesBytes: 3_749_307_417,
    imagesFound: 6,
    homeBytes: 24_118_000,
    runtimeBytes: all ? 4_220_000_000 : null,
    builtinRuntime: true,
    runtimeBlocked: all
      ? null
      : '这台电脑上 Hunter 跑在它自己的一台虚拟机里，你的数据就存在那台虚拟机的磁盘里。删掉运行环境等于把数据一起删掉 —— 而你选的是「保留数据」。',
    tableCount: all ? 23 : null,
    lastWrite: all ? '2026-09-23 09:41:02' : null,
    backups: 3,
    lastBackupAt: '2026-09-23 00:00:03',
    backupDir: '<用户目录>/Hunter-backups',
    scheduleInstalled: true,
    estFreedBytes: all ? 4_075_000_000 : 24_118_000,
    warnings: all ? ['备份目录 <用户目录>/Hunter-backups 不会被删除。'] : [],
    lines: ['本项目名下有 6 个容器', '本项目名下有 6 个数据卷（判据是 docker 的 com.docker.compose.project 标签，不是名字前缀）'],
  }
}

export const demoAlerts: MonitorAlerts = {
  at: '2026-09-23 10:12:40',
  alerts: [
    {
      id: 'host-disk',
      level: 'warn',
      title: '系统盘空间偏紧',
      detail: '这块盘只剩 12.4 GB。Hunter 的镜像与数据加起来有几个 GB，盘满了容器会起不来、数据库也可能写不进去。',
      facts: [
        '系统盘（/）剩 12.4 GB / 共 207 GB',
        '阈值：低于 20 GB 提醒、低于 5 GB 严重',
        'Hunter 自己能安全清掉的：旧镜像 1.8 GB + 悬空卷 0 B + 过期备份 654 kB = 1.8 GB',
      ],
      actions: ['cleanup_project_space'],
      advice: ['系统盘上别的文件（下载、废纸篓、旧的虚拟机镜像）要不要清，由你自己决定 —— 启动器不会去动 Hunter 之外的任何文件。'],
      needsAi: false,
    },
    {
      id: 'restart-api',
      level: 'warn',
      title: 'api 在反复重启',
      detail: 'api 这个服务在最近 60 分钟里重启了 4 次。反复重启通常说明它每次起来都撞到同一个问题。',
      facts: ['docker inspect 的 RestartCount 现在是 204', '窗口 60 分钟内的增量：4 次（阈值 3 次）'],
      actions: [],
      advice: ['点「让 AI 帮我看看」会把这个服务最近的日志（脱敏）交给诊断助手。'],
      needsAi: true,
    },
  ],
  notify: ['host-disk'],
  reasons: {},
}

export const demoCleanupPlan: CleanupPlan = {
  oldImages: [
    { reference: 'ghcr.io/agentpit-io/hunter-community-api:1.1.0', id: 'sha256:1a2b', bytes: 1_800_000_000 },
  ],
  oldImagesBytes: 1_800_000_000,
  orphanVolumes: [],
  orphanVolumesBytes: 0,
  expiredBackups: ['hunter-20260919-0000-v1.2.0'],
  expiredBackupsBytes: 654_812,
  totalBytes: 1_800_654_812,
  lines: ['现在装的是 v1.2.0；本项目别的版本的镜像有 1 个没有任何容器在用'],
  reasons: [],
}

export const demoOffline: OfflineImport = {
  path: '/home/user/hunter-images-1.2.0.tar',
  bytes: 1_893_400_576,
  seconds: 74,
  loaded: [
    'ghcr.io/agentpit-io/hunter-community-web:1.2.0',
    'ghcr.io/agentpit-io/hunter-community-api:1.2.0',
    'ghcr.io/agentpit-io/hunter-community-opencode:1.2.0',
    'ghcr.io/agentpit-io/hunter-community-llm-shim:1.2.0',
    'postgres:16-alpine',
    'redis:7-alpine',
  ],
  matched: [
    { service: 'web', reference: 'ghcr.io/agentpit-io/hunter-community-web:1.2.0', bytes: 240_000_000 },
    { service: 'api', reference: 'ghcr.io/agentpit-io/hunter-community-api:1.2.0', bytes: 960_000_000 },
    { service: 'opencode', reference: 'ghcr.io/agentpit-io/hunter-community-opencode:1.2.0', bytes: 812_000_000 },
    { service: 'llm-shim', reference: 'ghcr.io/agentpit-io/hunter-community-llm-shim:1.2.0', bytes: 48_000_000 },
    { service: 'postgres', reference: 'postgres:16-alpine', bytes: 92_000_000 },
    { service: 'redis', reference: 'redis:7-alpine', bytes: 18_000_000 },
  ],
  missing: [],
  registryPrefix: DEMO_REGISTRY,
  basePrefix: 'docker.io/library',
  tag: DEMO_LATEST_TAG,
  complete: true,
}

/**
 * 「今日额度已经用完」的运行面板演示态（截图脚本 `HUNTER_DEMO_PAGE=dashboard-quota-exhausted`）。
 *
 * 数字取自 I3 回归当天网关的真实响应：`used_today` 已经**超过** `limit_daily`
 * （304,144 / 300,000）—— 最后一次对话是在额度耗尽前发起的，算完才超。
 * 进度条因此必须夹住，不能画出格子。
 */
export const demoRuntimeQuotaExhausted: RuntimeStatus = {
  ...demoRuntime,
  quota: {
    usedToday: 304144,
    limitDaily: 300000,
    remaining: 0,
    exhausted: true,
    resetAt: '2026-09-21T00:00:00+08:00',
    rpm: 20,
    concurrency: 4,
  },
}

/**
 * 「这台机器是从 0.1.7 之前升上来的，网页端口还对局域网开着」的运行面板演示态
 * （截图脚本 `HUNTER_DEMO_PAGE=dashboard-lan`）。
 *
 * 新装的机器**不可能**是这个样子 —— 免费版只允许本机访问（I7 · 用户 2026-09-21 19:05 的决定）。
 * 绑定地址照着旧版本装出来的现场写：web 在 `0.0.0.0` 上，其余四个当初就是 `127.0.0.1`。
 */
export const demoRuntimeLanExposed: RuntimeStatus = {
  ...demoRuntime,
  webLanExposed: true,
  services: demoRuntime.services.map((s) =>
    s.service === 'web' ? { ...s, bind: '0.0.0.0' } : s,
  ),
}

/**
 * AI 诊断助手的演示态（I4）。
 *
 * 演示的是**规则层**那一半 —— 它不花 token、不联网，演示出来是诚实的。
 * AI 那一层在演示模式下一律显示「设置里关着」的降级态：真去问网关要花用户的额度，
 * 而编一段模型回复放进演示数据就是在假装一个能力（红线 1）。
 */
export const demoAssist: AssistState = {
  enabled: true,
  hasKey: true,
  rule: {
    rule: 'docker-not-found',
    code: 'E_DOCKER_MISSING',
    title: '所有已知位置都没找到 docker',
    detail:
      '按已知位置挨个探过了，都没有可执行的 docker：\n\n' +
      '· [PATH] /usr/bin/docker — 没有这个文件\n' +
      '· [已知位置] /usr/local/bin/docker — 没有这个文件\n' +
      '· [已知位置] /snap/bin/docker — 没有这个文件\n\n' +
      '本进程拿到的 PATH：/usr/bin:/bin:/usr/sbin:/sbin\n',
    actions: [
      {
        id: 'probe_docker_path',
        kind: 'readOnly',
        title: '重新按已知位置找一遍 docker',
        why: 'macOS 的 GUI 程序拿不到终端里的 PATH，得按已知安装位置挨个探',
        argv: [],
        summary: '按已知位置重新探测 docker，不改动任何东西',
      },
    ],
    confident: false,
  },
  turns: [],
  pending: [],
  totalTokens: 0,
  rounds: 0,
  maxRounds: 3,
  degraded: null,
  done: false,
  // 演示数据里也带上 I9 那两个字段 —— 形状要和真实的一模一样
  canResumeInstall: false,
  autoRan: [
    {
      id: 'start_builtin_runtime',
      title: '启动内置运行时的虚拟机',
      ok: true,
      text: '演示数据：内置运行时已就绪（profile hunter，用时 62 秒）',
    },
  ],
  reportText:
    '系统: linux x86_64 Ubuntu 24.04.3 LTS\n启动器: ' +
    DEMO_LAUNCHER_VERSION +
    '\n错误码: E_DOCKER_MISSING\n（演示数据，不是这台机器的真实现场）\n',
}


/**
 * I9 · 内置运行时残骸那一页的诊断快照。
 *
 * 界面上要同时看见两件事：
 *  ① 规则层认出来的是「Hunter 自己那台虚拟机没起来」，不是「你的 Docker 没开」；
 *  ② **启动器已经自己做了哪几步** —— 全自动档下不再让用户点，但做了什么必须看得见。
 */
export const demoAssistBuiltin: AssistState = {
  enabled: true,
  hasKey: true,
  rule: {
    rule: 'builtin-runtime-down',
    code: 'E_BUILTIN_DOWN',
    title: 'Hunter 自己那台虚拟机没起来',
    detail:
      'Hunter 自己那套运行时已经装好了（在 ~/.hunter/runtime 里），只是它的虚拟机没在跑。\n' +
      '这不是你电脑上的 Docker，也不是你装的 Colima —— 启动器不会去碰你的 ~/.colima。\n' +
      '把 profile hunter 这台虚拟机起起来就行，系统镜像本机已经有、校验过，不用下东西。\n' +
      '（判据：看 ~/.hunter/runtime 下的四件工具 + socket 连不连得上（装好了，虚拟机没起来））',
    actions: [
      {
        id: 'start_builtin_runtime',
        kind: 'mutating',
        title: '启动内置运行时的虚拟机',
        why: '内置运行时装好了，但它的虚拟机没在跑',
        argv: [
          '~/.hunter/runtime/bin/colima',
          'start',
          '--profile',
          'hunter',
          '--vm-type',
          'vz',
          '--cpu',
          '4',
          '--memory',
          '4',
          '--disk',
          '60',
        ],
        summary: null,
      },
    ],
    confident: true,
  },
  turns: [],
  pending: [],
  totalTokens: 0,
  rounds: 0,
  maxRounds: 3,
  degraded: null,
  done: false,
  canResumeInstall: false,
  autoRan: [
    {
      id: 'repair_builtin_runtime',
      title: '清掉上次没装成功的残骸再重建',
      ok: true,
      text: '演示数据：已清掉 ~/.hunter/runtime/colima/hunter；已清掉 ~/.hunter/runtime/lima/colima-hunter',
    },
    {
      id: 'start_builtin_runtime',
      title: '启动内置运行时的虚拟机',
      ok: false,
      text: '演示数据：colima 说起好了，但 ~/.hunter/runtime/colima/hunter/docker.sock 这个 socket 没出现 —— 不当成成功。',
    },
  ],
  reportText:
    '当前生效运行时：内置运行时（Colima profile hunter）（装好了，虚拟机没起来，socket ~/.hunter/runtime/colima/hunter/docker.sock 连不上）\n' +
    '（演示数据，不是这台机器的真实现场）\n',
}

// ── I5 · AI 自动驾驶安装的演示数据（只在 VITE_DEMO=1 的开发构建里存在） ──
//
// 这一份是**给截图脚本用的静态样例**，形状与真实事件流一致。
// 发布构建里 DEMO 是编译期常量 false，整个文件被摇掉，不可能进产物（红线 1）。

export const demoAutoSnapshot: AssistAutoSnapshot = {
  running: true,
  summary: {
    solved: 1,
    open: 0,
    tokens: 0,
    rounds: 1,
    maxRounds: 10,
    elapsedMs: 214_000,
    phase: '启动服务',
  },
  events: [
    {
      id: 0,
      parent: null,
      kind: 'step',
      status: 'ok',
      title: '检查 Docker',
      detail: 'OrbStack 29.4.0 正在运行',
      elapsedMs: 254,
      at: '2026-09-21 10:30:02',
    },
    {
      id: 2,
      parent: null,
      kind: 'step',
      status: 'ok',
      title: '挑下载源、算端口、写配置',
      detail: '下载源 腾讯云 · 香港 · 端口 web 3101 · api 8101 · opencode 3922 · postgres 5443 · redis 6480',
      elapsedMs: 6_100,
      at: '2026-09-21 10:30:08',
    },
    {
      id: 3,
      parent: 2,
      kind: 'issue',
      status: 'warn',
      title: '你电脑上已经在运行另一套 Hunter',
      detail: 'hunter-fresh（6 个容器，占着端口 3100、3921、5442、6479、8100）',
      tech: ['处置：新装的这一套换一组空闲端口，两套并存。启动器不会停它、不会删它。'],
      at: '2026-09-21 10:30:03',
    },
    {
      id: 4,
      parent: 2,
      kind: 'resolved',
      status: 'ok',
      title: '为这次安装换了一组空闲端口',
      detail: 'web 3100 → 3101 · api 8100 → 8101 · opencode 3921 → 3922 · postgres 5442 → 5443 · redis 6479 → 6480',
      at: '2026-09-21 10:30:07',
    },
    {
      id: 5,
      parent: 4,
      kind: 'analyze',
      status: 'ok',
      title: '8100 原来被占着',
      detail: 'Docker 容器 hunter-fresh-api-1（compose 项目 hunter-fresh · 0.0.0.0:8100->8000/tcp）',
      tech: [
        'Docker 容器 hunter-fresh-api-1（compose 项目 hunter-fresh · 0.0.0.0:8100->8000/tcp）',
        '绑 0.0.0.0 时被系统拒绝（Address already in use (os error 48)）',
      ],
      at: '2026-09-21 10:30:07',
    },
    {
      id: 6,
      parent: null,
      kind: 'step',
      status: 'ok',
      title: '下载组件',
      detail: '849 MB · 用时 3 分 27 秒',
      elapsedMs: 207_000,
      at: '2026-09-21 10:33:35',
    },
    {
      id: 7,
      parent: null,
      kind: 'step',
      status: 'running',
      title: '启动服务',
      detail: '5 / 6 健康',
      at: '2026-09-21 10:33:40',
    },
  ],
}

/**
 * 复核卡片 + 「直接用它」那张不阻塞的选择卡片长什么样（I7）。给截图脚本用
 * （`HUNTER_DEMO_PAGE=auto-review`）。
 *
 * **里面的数字都是从测试机真实跑出来的那一次抄过来的**（见 I7-迭代报告第三节），
 * 不是随手编的 —— 演示数据也不该出现一个真实系统里不可能出现的值。
 */
export const demoAutoReview: AssistAutoSnapshot = {
  running: true,
  summary: {
    solved: 0,
    open: 1,
    tokens: 2_146,
    rounds: 1,
    maxRounds: 10,
    elapsedMs: 18_400,
    phase: '正在解决：找不到 Docker',
  },
  events: [
    {
      id: 1,
      parent: null,
      kind: 'step',
      status: 'failed',
      title: '检查 Docker',
      detail: '这台机器上找不到 docker 可执行文件。',
      elapsedMs: 410,
      at: '2026-09-21 18:20:02',
    },
    {
      id: 2,
      parent: null,
      kind: 'issue',
      status: 'warn',
      title: '发现问题：找不到 Docker',
      detail: '这台机器上找不到 docker 可执行文件。',
      tech: ['错误码 E_DOCKER_MISSING｜这台机器上找不到 docker 可执行文件。'],
      at: '2026-09-21 18:20:02',
    },
    {
      id: 3,
      parent: 2,
      kind: 'analyze',
      status: 'ok',
      title: '分析中…',
      detail: '查了 5 个端口 · 本机还有 1 套 Hunter',
      elapsedMs: 1_240,
      at: '2026-09-21 18:20:03',
    },
    {
      id: 4,
      parent: 2,
      kind: 'analyze',
      status: 'ok',
      title: '找到原因',
      detail:
        '这台机器上一个 Docker 都没有（内置清单里 14 个位置全找过了）。装一套完全放在 ~/.hunter/runtime 里的运行时：4 个组件、合计 94.9 MiB，不要管理员密码、不改系统任何地方、设置里可一键卸载。',
      at: '2026-09-21 18:20:03',
    },
    {
      id: 5,
      parent: 2,
      kind: 'review',
      status: 'ok',
      title: '复核：放行',
      detail:
        '所有改动都落在 ~/.hunter/runtime 内，不触及用户已有的数据卷、网络设置或其他 compose 项目；理由与证据里「一个 docker 位置都没探到」一致。',
      tokens: 2_146,
      elapsedMs: 3_910,
      tech: [
        '第二个模型，单独的提示词，只判「会不会伤到你已有的东西」「对不对得上证据」',
        '复核通过 ≠ 可以执行：守卫与你的确认这两道照样要过',
      ],
      at: '2026-09-21 18:20:07',
    },
    {
      id: 6,
      parent: 2,
      kind: 'action',
      status: 'ok',
      title: '这一步不再问你',
      detail: '你在授权页上勾了「电脑上没有 Docker 时允许 AI 为你安装」',
      tech: [
        '动作 install_runtime（需要你同意）。想改回「每次都问」：设置页把那一项取消勾选，或者改 launcher.toml 的 [assist] allow_install_runtime = false',
      ],
      at: '2026-09-21 18:20:07',
    },
    {
      id: 7,
      parent: 2,
      kind: 'action',
      status: 'running',
      title: '正在下载容器运行时（4 个组件，合计 94.9 MiB）',
      detail: '38% · 已下载 36.1 MiB / 94.9 MiB',
      at: '2026-09-21 18:20:22',
    },
  ],
}

/**
 * I8：「你电脑上已经在运行另一套 Hunter」——**自己拿主意，不问用户**。
 *
 * I7 这里是一张不阻塞的选择卡片，两个按钮。用户 2026-09-21 22:35 把
 * 「让用户参与决策」整类做法否了，所以现在是一张**告知**卡片：
 * 做了什么决定、为什么这么定、想改的话去哪儿改。没有按钮。
 *
 * 同一张图里还有另外两条 I8 的东西：自动换下载源、自动沿用系统代理，
 * 各自都算进「已自动解决 N 个问题」。
 *
 * `HUNTER_DEMO_PAGE=auto-takeover-offer`。
 */
export const demoAutoTakeoverOffer: AssistAutoSnapshot = {
  running: true,
  summary: {
    solved: 3,
    open: 0,
    tokens: 0,
    rounds: 0,
    maxRounds: 10,
    elapsedMs: 29_400,
    phase: '挑下载源、算端口、写配置',
  },
  events: [
    {
      id: 1,
      parent: null,
      kind: 'step',
      status: 'ok',
      title: '检查 Docker',
      detail: 'Docker Engine 29.8.1 正在运行',
      elapsedMs: 254,
      at: '2026-09-21 18:30:02',
    },
    {
      id: 2,
      parent: null,
      kind: 'step',
      status: 'running',
      title: '挑下载源、算端口、写配置',
      detail: '正在测速…',
      at: '2026-09-21 18:30:03',
    },
    {
      id: 3,
      parent: 2,
      kind: 'resolved',
      status: 'ok',
      title: '你电脑上已经在运行另一套 Hunter',
      detail:
        'hunter-community（6 个容器，占着端口 3100、3921、5442、6479、8100）。已自动选择「和它并存」：新装的这一套换一组空闲端口，你原来那套一点都不动 —— 这是风险最小的做法。想改成直接使用已有那套，到「设置 → 已有的 Hunter」里切换。',
      tech: [
        '并存的做法：新装的这一套换一组空闲端口。启动器不会停它、不会删它、不会改它的配置。',
        '「hunter-community」的 compose 文件在 ~/hunter-community，所以在设置里切过去之后能看状态、看日志，也能停 / 重启（每次都要再确认一遍）',
      ],
      at: '2026-09-21 18:30:04',
    },
    {
      id: 4,
      parent: 2,
      kind: 'resolved',
      status: 'ok',
      title: '有 1 个下载源连不上，已自动改用「腾讯云 · 香港」',
      detail: '你不用做任何事，启动器自己挑了一个测得通的源',
      tech: ['GHCR · GitHub（请求 ghcr.io 失败：timeout: connect）'],
      at: '2026-09-21 18:30:24',
    },
    {
      id: 5,
      parent: 2,
      kind: 'resolved',
      status: 'ok',
      title: '直连 ghcr.io、raw.githubusercontent.com 不通，已沿用你设置的网络代理',
      detail: '检测到你设置了网络代理（HTTPS 127.0.0.1:7897 · HTTP 127.0.0.1:7897，来自系统设置），已沿用',
      tech: ['只读取你系统里已有的代理设置，不会修改它，也不会改 DNS / hosts / 防火墙。'],
      at: '2026-09-21 18:30:26',
    },
  ],
}

export const demoAudit: string[] = [
  '{"at":"2026-09-21 10:30:00","action":"consent","args":{},"by":"user","level":null,"result":"授权档位 auto（自动驾驶）"}',
  '{"at":"2026-09-21 10:30:01","action":"autopilot_start","args":{},"by":"user","level":null,"result":"授权档位 auto"}',
  '{"at":"2026-09-21 10:30:07","action":"remap_ports","args":{},"by":"rule","level":"Safe","result":"成功：web：3100 → 3101"}',
]


/** 「需要你」那张卡片长什么样（三种必问情况之一）。给截图脚本用。 */
export const demoAutoNeedUser: AssistAutoSnapshot = {
  running: true,
  summary: {
    solved: 0,
    open: 1,
    tokens: 3_880,
    rounds: 1,
    maxRounds: 10,
    elapsedMs: 21_000,
    phase: '正在解决：Docker 装了，但没在运行',
  },
  events: [
    {
      id: 1,
      parent: null,
      kind: 'step',
      status: 'failed',
      title: '检查 Docker',
      detail: 'docker 命令在，但连不上后台服务（daemon 没起）。',
      elapsedMs: 320,
      at: '2026-09-21 10:30:02',
    },
    {
      id: 2,
      parent: null,
      kind: 'issue',
      status: 'warn',
      title: '发现问题：Docker 装了但没在运行',
      detail: 'docker 命令在，但连不上后台服务（daemon 没起）。',
      tech: ['错误码 E_DAEMON_DOWN｜docker 命令在，但连不上后台服务（daemon 没起）。'],
      at: '2026-09-21 10:30:02',
    },
    {
      id: 3,
      parent: 2,
      kind: 'analyze',
      status: 'ok',
      title: '分析中…',
      detail: '查了 5 个端口',
      elapsedMs: 1_100,
      at: '2026-09-21 10:30:03',
    },
    {
      id: 4,
      parent: 2,
      kind: 'analyze',
      status: 'ok',
      title: '找到原因',
      detail: 'systemd 的 docker 服务 装着但没在运行，把它启动起来再等它就绪。',
      at: '2026-09-21 10:30:03',
    },
    {
      id: 5,
      parent: 2,
      kind: 'action',
      status: 'failed',
      title: '正在处理：启动 systemd 的 docker 服务',
      detail: '「启动 systemd 的 docker 服务」没成功（退出码 Some(1)）',
      elapsedMs: 180,
      at: '2026-09-21 10:30:04',
    },
    {
      id: 6,
      parent: 2,
      kind: 'needUser',
      status: 'waiting',
      title: 'Docker 后台服务要用管理员权限才能启动',
      detail: '请在终端里执行：sudo systemctl start docker',
      tech: ['这一条要管理员权限。启动器不会替你提权，也不会把它塞进动作表。'],
      choices: [
        { value: 'yes', label: '我执行完了，继续', primary: true },
        { value: 'no', label: '先不弄了', primary: false },
      ],
      at: '2026-09-21 10:30:04',
    },
  ],
}

// ── 现状复查（I11 · U1）──────────────────────────────────────────────────

const DEMO_SERVICES: ServiceStatus[] = [
  { service: 'web', state: 'running', health: 'healthy', port: 3100, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-web:${DEMO_HUNTER_TAG}` },
  { service: 'api', state: 'running', health: 'healthy', port: 8100, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-api:${DEMO_HUNTER_TAG}` },
  { service: 'opencode', state: 'running', health: 'healthy', port: 3921, bind: '127.0.0.1', exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-opencode:${DEMO_HUNTER_TAG}` },
  { service: 'llm-shim', state: 'running', health: 'healthy', port: null, bind: null, exitCode: null , image: `ghcr.io/agentpit-io/hunter-community-llm-shim:${DEMO_HUNTER_TAG}` },
  { service: 'postgres', state: 'running', health: 'healthy', port: 5442, bind: '127.0.0.1', exitCode: null , image: `docker.io/library/postgres:16-alpine` },
  { service: 'redis', state: 'running', health: 'healthy', port: 6479, bind: '127.0.0.1', exitCode: null , image: `docker.io/library/redis:7-alpine` },
]

/**
 * 截图用的复查结果。
 *
 * **默认给的是「还没好」那一档**：错误页那张图要能截得出来 ——
 * 给「已经好了」的话，页面会立刻自己跳去运行面板，截图脚本什么都截不到。
 * 想看「已经好了」的样子用 `__HUNTER_DEMO_PAGE__ = 'error-recovered'`。
 */
export function demoSelfCheck(page: string | null): SelfCheckReview {
  if (page === 'error-recovered') {
    return {
      posture: 'healthy',
      services: DEMO_SERVICES,
      ready: 6,
      total: 6,
      unready: [],
      missing: [],
      webUrl: 'http://localhost:3100',
      webStatus: 200,
      webReason: null,
      drift: [],
      headline: 'Hunter 已经在正常运行（6 / 6 健康，网页返回 HTTP 200）',
      lines: ['web · running · 健康', 'GET http://127.0.0.1:3100/ → HTTP 200'],
      elapsedMs: 412,
    }
  }
  const services = DEMO_SERVICES.map((s) =>
    s.service === 'opencode' ? { ...s, health: 'unhealthy' as const } : s,
  )
  return {
    posture: 'partial',
    services,
    ready: 5,
    total: 6,
    unready: ['opencode'],
    missing: [],
    webUrl: 'http://localhost:3100',
    webStatus: 200,
    webReason: null,
    drift: [],
    headline: 'Hunter 在跑，但 opencode 还没就绪',
    lines: ['opencode · running · 不健康', 'GET http://127.0.0.1:3100/ → HTTP 200'],
    elapsedMs: 388,
  }
}


// ── I12 · 资源监控 / 启停 / 已有数据的演示值 ─────────────────────────────
//
// 这一组**只**在 VITE_DEMO=1 的开发构建里存在（截图脚本用），发布包里被摇掉。
// 数字都取得像真的，但界面右上角有「演示数据」角标，不会被误当成实测值（红线 1）。

export const demoHost: HostMetrics = {
  cpuPct: 18.4,
  cpuCores: 10,
  memUsedBytes: 12_025_908_838,
  memTotalBytes: 17_179_869_184,
  memPressure: '正常',
  memPressureLevel: 'ok',
  diskFreeBytes: 239_483_392_000,
  diskTotalBytes: 994_662_584_320,
  diskMount: '/System/Volumes/Data',
  diskLevel: 'ok',
  reasons: {},
}

export const demoRuntimeMetrics: RuntimeMetrics = {
  applicable: true,
  reason: '',
  cpus: 4,
  memTotalBytes: 4_294_967_296,
  memUsedBytes: 2_254_857_830,
  memLevel: 'ok',
  diskUsedBytes: 8_482_488_320,
  diskTotalBytes: 64_424_509_440,
  diskLevel: 'ok',
  reasons: {},
}

export const demoServiceMetrics: ServiceMetrics = {
  reason: '',
  services: [
    { service: 'web', cpuPct: 1.2, memBytes: 141_557_760, memLimitBytes: 4_294_967_296, restartCount: 0, oomKilled: false },
    { service: 'api', cpuPct: 0.5, memBytes: 126_877_696, memLimitBytes: 4_294_967_296, restartCount: 0, oomKilled: false },
    { service: 'opencode', cpuPct: 8.9, memBytes: 272_629_760, memLimitBytes: 4_294_967_296, restartCount: 1, oomKilled: false },
    { service: 'llm-shim', cpuPct: 0.1, memBytes: 31_457_280, memLimitBytes: 4_294_967_296, restartCount: 0, oomKilled: false },
    { service: 'postgres', cpuPct: 0.3, memBytes: 48_234_496, memLimitBytes: 4_294_967_296, restartCount: 0, oomKilled: false },
    { service: 'redis', cpuPct: 0.2, memBytes: 9_961_472, memLimitBytes: 4_294_967_296, restartCount: 0, oomKilled: false },
  ],
}

const DEMO_VOLUMES = [
  { name: 'hunter_hunter_pg_data', short: 'hunter_pg_data', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_pg_data/_data', sizeBytes: 327_000_000 },
  { name: 'hunter_hunter_secrets', short: 'hunter_secrets', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_secrets/_data', sizeBytes: 12_000 },
  { name: 'hunter_hunter_opencode_data', short: 'hunter_opencode_data', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_opencode_data/_data', sizeBytes: 41_000_000 },
  { name: 'hunter_hunter_user_skills', short: 'hunter_user_skills', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_user_skills/_data', sizeBytes: 0 },
  { name: 'hunter_hunter_packages', short: 'hunter_packages', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_packages/_data', sizeBytes: 11_000_000 },
  { name: 'hunter_hunter_redis_data', short: 'hunter_redis_data', labelMissing: false, mountpoint: '/var/lib/docker/volumes/hunter_hunter_redis_data/_data', sizeBytes: 1_100_000 },
]

export const demoStorage: StorageMetrics = {
  volumes: DEMO_VOLUMES,
  volumesTotalBytes: 380_112_000,
  dbBytes: 327_000_000,
  imagesBytes: 4_003_000_000,
  imagesFound: 6,
  reasons: {},
}

export const demoDataCheck: DataCheck = {
  decision: 'reuse',
  volumes: DEMO_VOLUMES,
  hasDb: true,
  hasSecrets: true,
  hasJwtSecret: true,
  pgVersion: '16',
  targetPgVersion: '16',
  migrationMax: null,
  targetMigrationMax: null,
  tableCount: null,
  lastWrite: null,
  backups: 2,
  backupsWithSecrets: 2,
  deep: false,
  deepSkipped: null,
  headline: '检测到你以前的数据，将直接沿用，不会重建数据库、也不会重新生成密钥。',
  lines: [
    '数据卷 hunter_hunter_pg_data · hunter_pg_data · 数据库：账号、配置、自选股、历史分析',
    '数据卷 hunter_hunter_secrets · hunter_secrets · 密钥卷：数据库里加密的配置靠它才解得开',
    '~/.hunter/app/.env 里的 JWT_SECRET：在（重装会原样沿用，登录不会失效）',
    '数据卷里的 PostgreSQL 大版本：16',
  ],
  elapsedMs: 1841,
}

export const demoDataCheckDeep: DataCheck = {
  ...demoDataCheck,
  deep: true,
  tableCount: 61,
  lastWrite: '2026-09-22 20:22',
  migrationMax: '0022_schema_migrations.sql',
  targetMigrationMax: '0022_schema_migrations.sql',
  headline: '检测到你以前的数据（61 张表，最近更新于 2026-09-22 20:22），将直接沿用，不会重建数据库、也不会重新生成密钥。',
  elapsedMs: 14_203,
}

export const demoStackPlan: StackPlan = {
  builtinRunning: true,
  builtinMemGb: 4,
  services: ['web', 'api', 'opencode', 'llm-shim', 'postgres', 'redis'],
}

export function demoStackOp(action: 'stop' | 'start' | 'restart'): StackOpResult {
  const steps =
    action === 'stop'
      ? ['六个服务已经停下来（容器、数据卷、配置全都留着）', '内置运行时的虚拟机已停（用时 6 秒，释放约 4 GB 内存）']
      : action === 'start'
        ? ['运行环境（虚拟机）没在跑，先把它起起来', '内置运行时已就绪（profile hunter，用时 21 秒）', '六个容器已经起来，正在等它们变健康', '6 / 6 健康']
        : ['六个服务都重启过了']
  return {
    headline: action === 'stop' ? '这台机器上的 Hunter 已经停下来' : 'Hunter 已经在正常运行（6 / 6 健康，网页返回 HTTP 200）',
    steps,
    ready: action === 'stop' ? 0 : 6,
    total: 6,
    elapsedMs: action === 'start' ? 34_120 : 7_410,
  }
}

/**
 * 一键上传日志的演示态（I16）。
 *
 * **正文里一个真东西都没有**：key 是打过码的形状、路径以 `~` 开头、
 * 用户名与主机名换成了占位符 —— 演示数据也要长成「脱敏之后」的样子，
 * 否则这一屏就教会了用户「原来它会把这些发出去」。
 */
export const demoLogshipPreview: LogshipPreview = {
  sections: demoDiagSections,
  body:
    '===== 概要 (summary) =====\n' +
    '时间: 2026-09-25 10:12:03\n启动器: 0.1.16\n系统: macos aarch64\n' +
    'Docker: OrbStack · server 29.8.1 · compose 5.5.1\nHunter tag: 1.2.0\n' +
    '镜像源: tencent (hkccr.ccs.tencentyun.com/agentpit)\n\n' +
    '===== 启动器日志（最近 200 行） (launcher-log) =====\n' +
    '10:05:41 [info] 第 1 次拉取，镜像源 hkccr.ccs.tencentyun.com/agentpit\n' +
    '10:07:11 [warn] 拉取卡住：90 秒没有任何新数据进来，已经把 docker compose pull 杀掉\n',
  bodyBytes: 512,
  truncated: false,
  machineId: '11111111-2222-4333-8444-555555555555',
  endpoint: 'https://www.agentpit.io/api/v1/launcher/logs',
  stage: 'upgrade',
  errorCode: 'E_PULL_STALLED',
  summary: '升级时从腾讯云香港拉镜像 90 秒没有数据进来',
  metaLines: [
    '镜像源: tencent (hkccr.ccs.tencentyun.com/agentpit)',
    '这台机器配了网络代理: false',
    '磁盘剩余（GB）: 42',
  ],
  scanHit: null,
  autoOnError: false,
}

/** 上传成功之后那一屏（演示态）。追踪码是编的，界面上有「演示数据」角标。 */
export const demoLogshipOutcome: LogshipOutcome = {
  ok: true,
  traceCode: 'HL-DEMO01',
  truncated: false,
  message: '已上传。追踪码 HL-DEMO01，把它发给我们，我们就能查到这份日志。',
  status: 200,
  bundlePath: null,
  localLogPath: '~/.hunter/logs/launcher.log',
}

/** 「上一次升级没做完」的演示态 —— 就是客户 2026-09-25 强退之后那个现场。 */
export const demoInterruptedUpgrade: InterruptedUpgrade = {
  configTag: '1.2.2',
  runningTag: '1.2.0',
  missingImages: ['web', 'api', 'opencode'],
  headline: '上一次升级没做完：配置已经是 v1.2.2，跑着的还是 v1.2.0，而 v1.2.2 的镜像本机不齐',
  lines: [
    '配置（.env 与 compose）写的是 v1.2.2',
    '正在跑的 4 个容器用的是 v1.2.0',
    'v1.2.2 的镜像本机还缺 3 个：web、api、opencode',
    '所以上一次升级多半是拉镜像时被打断的（强退 / 断电 / 关机）。在你选之前，启动器不会按新配置去起容器 —— 那只会再卡一次。',
  ],
}
