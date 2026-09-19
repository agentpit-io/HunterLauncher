/** 界面里用到的少量图标，全部内联 SVG（离线可用，不引图标库）。 */

export function CheckCircle({ size = 15, className = '' }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 20 20" width={size} height={size} aria-hidden className={className}>
      <circle cx="10" cy="10" r="9" fill="currentColor" />
      <path d="M5.8 10.2 8.6 13 14.2 7.2" fill="none" stroke="var(--hl-bg-card)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

export function AlertTriangle({ size = 16, className = '' }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 20 20" width={size} height={size} fill="none" aria-hidden className={className}>
      <path d="M10 2.8 18.4 17H1.6L10 2.8Z" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" />
      <path d="M10 7.8v4.1" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
      <circle cx="10" cy="14.4" r="0.95" fill="currentColor" />
    </svg>
  )
}

export function Spinner({ size = 16, className = '' }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 20 20" width={size} height={size} aria-hidden className={`animate-spin ${className}`}>
      <circle cx="10" cy="10" r="8" fill="none" stroke="currentColor" strokeWidth="2" opacity="0.2" />
      <path d="M10 2a8 8 0 0 1 8 8" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  )
}

export function Diamond({ size = 14, className = '' }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} aria-hidden className={className}>
      <rect x="12" y="3" width="12.7" height="12.7" rx="2" transform="rotate(45 12 3)" fill="none" stroke="currentColor" strokeWidth="2" />
    </svg>
  )
}

/** 齿轮。标题栏右侧的设置入口用它（视觉稿里没有这个入口，按同一套令牌延展）。 */
export function Gear({ size = 16, className = '' }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} fill="none" stroke="currentColor" strokeWidth="1.7" className={className} aria-hidden>
      <circle cx="12" cy="12" r="3.1" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 8.9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  )
}
