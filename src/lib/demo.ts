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
  AppInfo,
  BackupMeta,
  BootState,
  DiagSection,
  DockerInfo,
  ImagePull,
  KeyCheckResult,
  LauncherSettings,
  LauncherUpdate,
  MissingEndpoint,
  OfflineImport,
  OwnKeyCheck,
  PullProgress,
  RegistryProbe,
  RuntimeStatus,
  TelemetryView,
  UpgradeCheck,
} from './types'

/** 构建产物里能被 grep 到的标记，CI 用它确认发布包里没有演示数据。 */
export const DEMO_MARKER = 'HUNTER_DEMO_DATA_MARKER'

export const DEMO_LAUNCHER_VERSION = '0.1.0'
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
  wsl: null,
  meetsMinimum: false,
  problem: '命令行里找不到 docker。',
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
  services: [
    { service: 'web', state: 'running', health: 'healthy', port: 3100, exitCode: null },
    { service: 'api', state: 'running', health: 'healthy', port: 8100, exitCode: null },
    { service: 'opencode', state: 'running', health: 'healthy', port: 3921, exitCode: null },
    { service: 'llm-shim', state: 'running', health: 'healthy', port: null, exitCode: null },
    { service: 'postgres', state: 'running', health: 'healthy', port: 5442, exitCode: null },
    { service: 'redis', state: 'running', health: 'healthy', port: 6479, exitCode: null },
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
  { service: 'postgres', state: 'running', health: 'healthy', port: 5442, exitCode: null },
  { service: 'redis', state: 'running', health: 'healthy', port: 6479, exitCode: null },
  { service: 'llm-shim', state: 'running', health: 'healthy', port: null, exitCode: null },
  { service: 'api', state: 'running', health: 'healthy', port: 8100, exitCode: null },
  { service: 'opencode', state: 'running', health: 'starting', port: 3921, exitCode: null },
  { service: 'web', state: 'created', health: 'pending', port: 3100, exitCode: null },
]

export const demoSettings: LauncherSettings = {
  locale: 'zh-CN',
  autostart: false,
  checkUpdate: true,
  // 默认对外 —— 和真实默认值一致（演示模式也不该显示一个不存在的默认）
  webLocalOnly: false,
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
  version: '0.1.1',
  notes: '修了拉取页在慢网络下 ETA 抖动的问题；升级失败回滚后不再重复提示。',
  date: '2026-09-21T02:00:00Z',
  // 演示的是 .deb 这一路 —— 它的界面分支更多（要显示那条命令），预览时更有用
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
