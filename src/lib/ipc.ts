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
  BootState,
  DockerInfo,
  KeyCheckResult,
  LauncherSettings,
  OwnKeyCheck,
  PullProgress,
  RegistryProbe,
  RuntimeStatus,
  ServiceStatus,
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
  if (DEMO) return demo.demoRuntime
  return call<RuntimeStatus>('runtime_status')
}

export async function stackAction(action: 'stop' | 'start' | 'restart' | 'down'): Promise<string> {
  if (DEMO) return '演示模式不真的操作容器'
  return call<string>('stack_action', { action })
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

export async function readSettings(): Promise<LauncherSettings> {
  if (DEMO) return demo.demoSettings
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

// ── 事件 ─────────────────────────────────────────────────────────────────

const EV_PULL = 'hunter://pull'
const EV_START = 'hunter://start'
const EV_LOG = 'hunter://log'

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

/** 准备阶段（选源 / 取 compose / 写配置）的逐行文字，拉取页底部的日志框显示它。 */
export function onLogLine(cb: (line: string) => void): Promise<UnlistenFn> {
  return on<string>(EV_LOG, cb)
}
