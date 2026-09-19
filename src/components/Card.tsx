import type { ReactNode } from 'react'

/** 视觉稿里所有卡片共用的底：#0f1626 底 + 1px #181f2d 描边 + 12px 圆角。 */
export function Card({
  children,
  className = '',
  padded = true,
}: {
  children: ReactNode
  className?: string
  padded?: boolean
}) {
  return (
    <div className={`rounded-lg border border-line bg-card ${padded ? 'p-card' : ''} ${className}`}>{children}</div>
  )
}

/** 卡片标题行：左边一个小标题，右边可以塞角标。 */
export function CardHead({ title, right }: { title: ReactNode; right?: ReactNode }) {
  return (
    <div className="flex h-[26px] items-center justify-between">
      <div className="text-md font-medium text-ink">{title}</div>
      {right}
    </div>
  )
}
