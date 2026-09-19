import type { ReactNode } from 'react'

type Tone = 'neutral' | 'amber' | 'success' | 'danger' | 'info'

const TONE: Record<Tone, string> = {
  // 视觉稿第 3 张的「有新版本 v1.0.0」：琥珀色描边 + 极淡的金底 + 金字
  amber: 'border-amber/45 bg-amber-soft text-amber-text',
  neutral: 'border-line-strong bg-card text-dim',
  success: 'border-success/40 bg-success-soft text-success',
  danger: 'border-danger/45 bg-danger-soft text-danger',
  info: 'border-info/40 bg-info-soft text-info',
}

export function Badge({ children, tone = 'neutral', className = '' }: { children: ReactNode; tone?: Tone; className?: string }) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-sm border px-2.5 py-1 text-xs leading-none ${TONE[tone]} ${className}`}
    >
      {children}
    </span>
  )
}
