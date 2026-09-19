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
