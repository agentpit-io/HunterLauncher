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
  BootState,
  BuiltinRuntimeStatus,
  DiagSection,
  AssistState,
  DockerInfo,
  ImagePull,
  KeyCheckResult,
  LauncherSettings,
  LauncherUpdate,
  MissingEndpoint,
  OfflineImport,
  OneClickFeedback,
  OwnKeyCheck,
  PullProgress,
  RegistryProbe,
  RuntimeStatus,
  TakeoverCandidate,
  TakeoverState,
  TelemetryView,
  UpgradeCheck,
} from './types'

/** 构建产物里能被 grep 到的标记，CI 用它确认发布包里没有演示数据。 */
export const DEMO_MARKER = 'HUNTER_DEMO_DATA_MARKER'

/**
 * 演示数据里的版本号。**发版时要跟着 `package.json` 一起改** ——
 * I8 之前它一直停在 `0.1.0`，于是每一轮的截图右上角都写着「启动器 v0.1.0」，
 * 看图的人分不清那是哪一版的界面（I8 截图时才发现）。
 */
export const DEMO_LAUNCHER_VERSION = '0.1.8'
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

export const demoBootState: BootState = {
  installed: false,
  running: false,
  locale: 'zh-CN',
  hunterTag: '1.2.0',
  workDir: '~/.hunter',
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
    { service: 'web', state: 'running', health: 'healthy', port: 3100, bind: '127.0.0.1', exitCode: null },
    { service: 'api', state: 'running', health: 'healthy', port: 8100, bind: '127.0.0.1', exitCode: null },
    { service: 'opencode', state: 'running', health: 'healthy', port: 3921, bind: '127.0.0.1', exitCode: null },
    { service: 'llm-shim', state: 'running', health: 'healthy', port: null, bind: null, exitCode: null },
    { service: 'postgres', state: 'running', health: 'healthy', port: 5442, bind: '127.0.0.1', exitCode: null },
    { service: 'redis', state: 'running', health: 'healthy', port: 6479, bind: '127.0.0.1', exitCode: null },
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
  { service: 'postgres', state: 'running', health: 'healthy', port: 5442, bind: '127.0.0.1', exitCode: null },
  { service: 'redis', state: 'running', health: 'healthy', port: 6479, bind: '127.0.0.1', exitCode: null },
  { service: 'llm-shim', state: 'running', health: 'healthy', port: null, bind: null, exitCode: null },
  { service: 'api', state: 'running', health: 'healthy', port: 8100, bind: '127.0.0.1', exitCode: null },
  { service: 'opencode', state: 'running', health: 'starting', port: 3921, bind: '127.0.0.1', exitCode: null },
  // 还没创建出来的容器 docker 报不出绑定地址 —— 那就是 null，不猜（红线 1）
  { service: 'web', state: 'created', health: 'pending', port: 3100, bind: null, exitCode: null },
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
  version: '0.1.9',
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

export const demoBackups: BackupMeta[] = [
  {
    id: '2026-09-20_031854-v1.1.0',
    tag: DEMO_HUNTER_TAG,
    at: '2026-09-20 03:18:54',
    sqlBytes: 1_482_301,
    sqlError: null,
    files: ['.env', 'docker-compose.yml', 'docker-compose.launcher.yml', 'launcher.toml'],
  },
]

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
  reportText:
    '系统: linux x86_64 Ubuntu 24.04.3 LTS\n启动器: ' +
    DEMO_LAUNCHER_VERSION +
    '\n错误码: E_DOCKER_MISSING\n（演示数据，不是这台机器的真实现场）\n',
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
