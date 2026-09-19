import type { ReactNode } from 'react'
import { Card } from './Card'

/**
 * 统计卡片（视觉稿第 1 张的三张、第 3 张的四张）。
 * 结构固定：小标签 → 大数值（+ 小单位）→ 副说明。
 * value 传 null 时显示「—」并把 reason 放到副说明里 —— 总控规则红线 1：拿不到就显示「—」并给原因。
 */
export function StatCard({
  label,
  value,
  unit,
  sub,
  reason,
  mono = true,
  children,
  className = '',
}: {
  label: ReactNode
  value: ReactNode | null
  unit?: ReactNode
  sub?: ReactNode
  reason?: string
  mono?: boolean
  children?: ReactNode
  className?: string
}) {
  const empty = value === null || value === undefined
  // 行高不能用 leading-none：truncate 带 overflow:hidden，行盒等于字号时中文字形的上下会被切掉
  return (
    <Card padded={false} className={`flex min-w-0 flex-col p-[18px] ${className}`}>
      <div className="truncate text-sm leading-[1.3] text-muted">{label}</div>
      <div className="mt-[9px] flex min-w-0 items-baseline gap-2">
        <span
          className={`truncate text-2xl leading-[1.15] ${mono && !empty ? 'tnum' : ''} ${empty ? 'text-muted' : 'text-ink'}`}
        >
          {empty ? '—' : value}
        </span>
        {!empty && unit && <span className="shrink-0 text-sm leading-[1.3] text-muted">{unit}</span>}
      </div>
      {children}
      {(sub || (empty && reason)) && (
        <div className="mt-[3px] truncate text-sm leading-[1.3] text-muted" title={empty ? reason : undefined}>
          {empty ? reason : sub}
        </div>
      )}
    </Card>
  )
}
