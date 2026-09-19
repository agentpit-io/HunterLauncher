import { useCallback, useEffect, useState } from 'react'
import { IpcError } from './types'

export interface AsyncError {
  code: string
  message: string
}

export interface AsyncState<T> {
  data: T | null
  loading: boolean
  /** 拿不到数据时的原因，界面把「—」和它一起显示（总控规则红线 1） */
  error: AsyncError | null
  reload: () => void
}

function toError(e: unknown): AsyncError {
  if (e instanceof IpcError) return { code: e.code, message: e.message }
  return { code: 'E_UNKNOWN', message: e instanceof Error ? e.message : String(e) }
}

/**
 * 跑一个 ipc 调用并把三种结果（加载中 / 有数据 / 拿不到且有原因）交给界面。
 * 关键点：失败时 data 保持 null —— 不做任何假数据兜底。
 */
export function useAsync<T>(fn: () => Promise<T>, deps: unknown[] = []): AsyncState<T> {
  const [snapshot, setSnapshot] = useState<{ data: T | null; loading: boolean; error: AsyncError | null }>({
    data: null,
    loading: true,
    error: null,
  })
  const [nonce, setNonce] = useState(0)

  useEffect(() => {
    let alive = true
    fn()
      .then((v) => {
        if (alive) setSnapshot({ data: v, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (alive) setSnapshot({ data: null, loading: false, error: toError(e) })
      })
    return () => {
      alive = false
    }
    // fn 每次渲染都是新的箭头函数，放进依赖会无限循环；真正的触发条件是 nonce 与调用方给的 deps
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nonce, ...deps])

  const reload = useCallback(() => {
    setSnapshot((s) => ({ ...s, loading: true }))
    setNonce((n) => n + 1)
  }, [])

  return { ...snapshot, reload }
}
