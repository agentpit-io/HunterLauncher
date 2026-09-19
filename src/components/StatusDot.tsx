import type { Health } from '../lib/types'

/**
 * 状态点。视觉稿里「健康 / 运行中」用的是琥珀金而不是绿色，这里照做；
 * 绿色留给「完成」这类一次性结果，红色给不健康。
 */
const TONE: Record<Health, string> = {
  healthy: 'bg-amber',
  starting: 'bg-amber/45',
  unhealthy: 'bg-danger',
  none: 'bg-line-strong',
  pending: 'bg-line-strong',
}

export function StatusDot({ health, size = 8, pulse = false }: { health: Health; size?: number; pulse?: boolean }) {
  return (
    <span
      aria-hidden
      style={{ width: size, height: size }}
      className={`inline-block shrink-0 rounded-full ${TONE[health]} ${pulse ? 'animate-pulse' : ''}`}
    />
  )
}
