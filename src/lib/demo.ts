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
  DockerInfo,
  ImagePull,
  KeyCheckResult,
  LauncherSettings,
  PullProgress,
  RuntimeStatus,
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
}

export const demoKeyCheck: KeyCheckResult = {
  valid: true,
  reason: 'ok',
  quota: {
    usedToday: 115,
    limitDaily: 300000,
    remaining: 299885,
    resetAt: '2026-09-20T00:00:00+08:00',
    rpm: 20,
    concurrency: 4,
  },
  models: [
    { id: 'hunter-chat', purpose: 'chat' },
    { id: 'hunter-deep', purpose: 'deep_analysis' },
  ],
}

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
  }
})

export const demoPull: PullProgress = {
  registry: DEMO_REGISTRY,
  images: demoImages,
  log: [
    '17:42:08 pull api layer 7/12 sha256:4c1e… extracting',
    '17:42:07 pull opencode layer 4/6 sha256:a9f2… downloading',
  ],
  etaSeconds: 80,
}

export const demoRuntime: RuntimeStatus = {
  hunterTag: DEMO_HUNTER_TAG,
  quota: {
    usedToday: 42300,
    limitDaily: 300000,
    remaining: 257700,
    resetAt: '2026-09-20T00:00:00+08:00',
    rpm: 20,
    concurrency: 4,
  },
  latestTag: DEMO_LATEST_TAG,
  running: true,
  uptimeSeconds: 3 * 86400 + 6 * 3600,
  webUrl: 'http://localhost:3100',
  services: [
    { service: 'web', state: 'running', health: 'healthy', port: 3100 },
    { service: 'api', state: 'running', health: 'healthy', port: 8100 },
    { service: 'opencode', state: 'running', health: 'healthy', port: 3921 },
    { service: 'llm-shim', state: 'running', health: 'healthy', port: null },
    { service: 'postgres', state: 'running', health: 'healthy', port: 5442 },
    { service: 'redis', state: 'running', health: 'healthy', port: 6479 },
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
}

export const demoStartingServices: RuntimeStatus['services'] = [
  { service: 'postgres', state: 'running', health: 'healthy', port: 5442 },
  { service: 'redis', state: 'running', health: 'healthy', port: 6479 },
  { service: 'llm-shim', state: 'running', health: 'healthy', port: null },
  { service: 'api', state: 'running', health: 'healthy', port: 8100 },
  { service: 'opencode', state: 'running', health: 'starting', port: 3921 },
  { service: 'web', state: 'created', health: 'pending', port: 3100 },
]

export const demoSettings: LauncherSettings = {
  locale: 'zh-CN',
  autostart: false,
  checkUpdate: true,
  registry: DEMO_REGISTRY,
  hunterTag: DEMO_HUNTER_TAG,
  workDir: '~/.hunter',
  telemetry: false,
}

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
