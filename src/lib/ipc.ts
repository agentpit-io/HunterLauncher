/*
 * 前端与 Rust 之间的唯一通道（技术方案附录 A：src/lib/ipc.ts）
 * ---------------------------------------------------------------------------
 * 两条路径：
 *   · 正常构建 → 真的 invoke 到 Tauri command。M1 的 Rust 侧只实现了 app_info，
 *     其余命令一律返回 E_NOT_IMPLEMENTED，界面因此显示「—」和原因（总控规则红线 1）。
 *   · VITE_DEMO=1 的开发构建 → 走 demo.ts 的演示数据，界面右上角显示「演示数据」角标。
 *
 * 除了 app_info，本文件不做任何降级：拿不到就抛 IpcError，由页面决定怎么显示「—」。
 * 绝不在这里用假数据兜底，那正是红线 1 禁止的事。
 */

import { invoke } from '@tauri-apps/api/core'
import { IpcError } from './types'
import type {
  AppInfo,
  DockerInfo,
  KeyCheckResult,
  LauncherSettings,
  PullProgress,
  RuntimeStatus,
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

// ── 基础信息（M1 已实现，真实数据） ──────────────────────────────────────

export async function appInfo(): Promise<AppInfo> {
  if (DEMO) return demo.demoAppInfo
  return call<AppInfo>('app_info')
}

// ── 窗口控制（M1 已实现） ────────────────────────────────────────────────

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

// ── 以下都是 M2 / M3 的活，M1 只留签名 ───────────────────────────────────

export async function detectDocker(): Promise<DockerInfo> {
  if (DEMO) return demoPage() === 'docker-missing' ? demo.demoDockerMissing : demo.demoDocker
  return call<DockerInfo>('detect_docker')
}

export async function validateKey(key: string): Promise<KeyCheckResult> {
  if (DEMO) return demo.demoKeyCheck
  return call<KeyCheckResult>('validate_key', { key })
}

export async function pullProgress(): Promise<PullProgress> {
  if (DEMO) return demo.demoPull
  return call<PullProgress>('pull_progress')
}

export async function runtimeStatus(): Promise<RuntimeStatus> {
  if (DEMO) return demo.demoRuntime
  return call<RuntimeStatus>('runtime_status')
}

export async function startingStatus(): Promise<RuntimeStatus> {
  if (DEMO) return { ...demo.demoRuntime, running: false, services: demo.demoStartingServices }
  return call<RuntimeStatus>('starting_status')
}

export async function readSettings(): Promise<LauncherSettings> {
  if (DEMO) return demo.demoSettings
  return call<LauncherSettings>('read_settings')
}

export async function launcherLog(tail: number): Promise<string[]> {
  if (DEMO) return demo.demoLauncherLog
  return call<string[]>('launcher_log', { tail })
}

export async function openExternal(url: string): Promise<void> {
  if (DEMO) return
  await call<void>('open_external', { url })
}
