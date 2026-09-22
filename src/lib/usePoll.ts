import { useEffect, useRef, useState } from 'react'
import { IpcError } from './types'

export interface PollState<T> {
  data: T | null
  /** 拿不到时的原因（界面把「—」和它一起显示，红线 1） */
  error: string | null
  /** 第一次还没回来 */
  loading: boolean
  /** 现在在不在采（窗口隐藏时是 false） */
  active: boolean
  reload: () => void
}

/**
 * 按固定周期拉一个只读指标，**窗口看不见就停**（I12 · R2 · 方案第六节第 5 条）。
 *
 * ## 为什么要「看不见就停」
 *
 * 这几条命令不是免费的：`docker stats --no-stream` 要起一个子进程、
 * `limactl shell` 要进一次虚拟机。用户把启动器收到托盘之后，每 5 秒起一个进程
 * 纯粹是在耗他的电 —— 而那时候界面上根本没有人在看这些数字。
 *
 * 判据用的是 `document.visibilityState`：窗口最小化、收进托盘、切到别的桌面
 * 都会变成 `hidden`。重新可见时**立刻补一次**，不让用户对着一屏旧数字。
 *
 * ## 失败了怎么办
 *
 * 保留上一次的 `data`（面板不该因为一次采样失败就整块闪成空白），
 * 同时把 `error` 置上 —— 具体哪一项要显示「—」由各自的 `reasons` 决定，
 * 那是后端给的、逐项的原因。这一层的 `error` 只说「这一轮整个没采到」。
 */
export function usePoll<T>(fn: () => Promise<T>, everyMs: number, enabled = true): PollState<T> {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [visible, setVisible] = useState(
    () => typeof document === 'undefined' || document.visibilityState !== 'hidden',
  )
  const [nonce, setNonce] = useState(0)
  // fn 每次渲染都是新的箭头函数；放进依赖会让定时器每次渲染都重建。
  // 存进 ref 这件事要在**副作用里**做 —— 渲染期间写 ref 是 React 明令禁止的
  const ref = useRef(fn)
  useEffect(() => {
    ref.current = fn
  })

  useEffect(() => {
    if (typeof document === 'undefined') return
    const on = () => setVisible(document.visibilityState !== 'hidden')
    document.addEventListener('visibilitychange', on)
    return () => document.removeEventListener('visibilitychange', on)
  }, [])

  const active = enabled && visible

  useEffect(() => {
    if (!active) return
    let alive = true
    let timer = 0
    const tick = () => {
      ref
        .current()
        .then((v) => {
          if (!alive) return
          setData(v)
          setError(null)
          setLoading(false)
        })
        .catch((e: unknown) => {
          if (!alive) return
          setError(e instanceof IpcError ? e.message : e instanceof Error ? e.message : String(e))
          setLoading(false)
        })
        .finally(() => {
          // 下一次从**这一次结束**开始算，而不是固定间隔 ——
          // 采一次要 3 秒的机器上，固定间隔会让请求排队越堆越多
          if (alive) timer = window.setTimeout(tick, everyMs)
        })
    }
    tick()
    return () => {
      alive = false
      window.clearTimeout(timer)
    }
  }, [active, everyMs, nonce])

  return { data, error, loading, active, reload: () => setNonce((n) => n + 1) }
}
