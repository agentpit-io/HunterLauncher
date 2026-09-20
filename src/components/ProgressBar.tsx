/**
 * 进度条。视觉稿第 2 张：进行中是琥珀金，已完成整条变成灰蓝（#3f5072），轨道 #17202f，高 6px。
 */
export function ProgressBar({
  value,
  done = false,
  tone = 'amber',
  height = 6,
  className = '',
}: {
  /** 0–100 */
  value: number
  done?: boolean
  /** 'danger' 用在「额度已经用完」这种条已经满了但不是好事的地方（I3） */
  tone?: 'amber' | 'danger'
  height?: number
  className?: string
}) {
  const pct = Math.max(0, Math.min(100, value))
  return (
    <div
      role="progressbar"
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
      style={{ height }}
      className={`w-full overflow-hidden rounded-full bg-slate-track ${className}`}
    >
      <div
        style={{ width: `${done ? 100 : pct}%` }}
        className={`h-full rounded-full transition-[width] duration-300 ease-out ${done ? 'bg-slate-done' : tone === 'danger' ? 'bg-danger' : 'bg-amber'}`}
      />
    </div>
  )
}
