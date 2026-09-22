import { useCallback, useEffect, useRef, useState } from 'react'
import * as ipc from './ipc'
import type { SelfCheckReview } from './types'

/** 两次自动复查之间隔多久。**低频**：这不是进度条，是「有没有人在外面把它修好了」。 */
export const POLL_MS = 20_000

export interface SelfCheck {
  review: SelfCheckReview | null
  /** 第一次还没回来 */
  loading: boolean
  /** 复查本身失败了（docker 没起来之类）。这不是「Hunter 坏了」，是「这次没问出来」 */
  error: string | null
  /** 立刻再查一次（AI / 规则层跑完一个动作之后调它） */
  refresh: () => void
}

/**
 * 「Hunter 现在到底在不在跑」——**持续低频复查**（I11 · U1）。
 *
 * 0.1.9 在用户 Mac 上的那一幕：21:39 报了 E_START_TIMEOUT，22:05 外部把根因
 * （虚拟机没有 DNS）修好、六个服务全绿、网页 200，而界面一直到 22:50
 * 还挂着 21:39 那张失败卡片。用户能做的只有点「重试」—— 点下去的结果是
 * 重新取 compose、重写 .env、重新下载 849 MB。
 *
 * 这个 hook 就是让界面自己去看一眼现实。它只读：后端那一侧是
 * `docker compose ps` + 一次本机 HTTP GET，不碰任何容器、配置、镜像。
 *
 * 触发时机三种，缺一不可：
 *   1. 挂载时立刻查一次 —— 用户可能是刚打开启动器，服务早就好了；
 *   2. 每 {@link POLL_MS} 毫秒查一次 —— 外部修好这件事没人会通知我们；
 *   3. 窗口重新拿到焦点时查一次 —— 用户去别处折腾了一圈回来，第一眼就该是对的。
 */
export function useSelfCheck(enabled = true): SelfCheck {
  const [review, setReview] = useState<SelfCheckReview | null>(null)
  // 没开的时候不存在「还在查」这回事 —— 初值直接算对，不靠 effect 里补一刀
  const [loading, setLoading] = useState(enabled)
  const [error, setError] = useState<string | null>(null)
  // 外面（AssistPanel 跑完动作）要能立刻叫一次。订阅期之外调它什么都不做
  const trigger = useRef<(() => void) | null>(null)

  useEffect(() => {
    if (!enabled) return
    let alive = true
    let inflight = false

    async function once() {
      // 上一次还没回来就别叠加 —— 复查要跑子进程，堆起来只会更慢
      if (inflight) return
      inflight = true
      try {
        const r = await ipc.selfCheck(false)
        if (!alive) return
        setReview(r)
        setError(null)
      } catch (e) {
        if (!alive) return
        // 查不出来就说查不出来，不拿上一次的结果冒充现在（红线 1）
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        inflight = false
        if (alive) setLoading(false)
      }
    }

    trigger.current = () => void once()
    void once()
    const timer = setInterval(() => void once(), POLL_MS)
    const onFocus = () => void once()
    window.addEventListener('focus', onFocus)
    return () => {
      alive = false
      trigger.current = null
      clearInterval(timer)
      window.removeEventListener('focus', onFocus)
    }
  }, [enabled])

  const refresh = useCallback(() => {
    trigger.current?.()
  }, [])

  return { review, loading, error, refresh }
}
