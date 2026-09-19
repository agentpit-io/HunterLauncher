import type { ButtonHTMLAttributes, ReactNode } from 'react'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger'
type Size = 'md' | 'sm'

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: Size
  /** 右侧图标，例如「下一步 ›」的箭头 */
  trailing?: ReactNode
  leading?: ReactNode
}

const VARIANT: Record<Variant, string> = {
  // 视觉稿：主按钮是琥珀金实心，文字接近黑
  primary: 'bg-amber text-on-amber hover:bg-amber-hover active:bg-amber-active font-medium',
  // 次按钮：透明底 + 描边，实测描边比卡片边框亮一档
  secondary: 'bg-transparent text-ink-2 border border-line-strong hover:bg-card hover:text-ink',
  ghost: 'bg-transparent text-body hover:text-ink hover:bg-card',
  danger: 'bg-transparent text-danger border border-danger/40 hover:bg-danger-soft',
}

const SIZE: Record<Size, string> = {
  md: 'h-[40px] px-5 text-md rounded-md',
  sm: 'h-[34px] px-4 text-sm rounded-md',
}

export function Button({ variant = 'secondary', size = 'md', trailing, leading, children, className = '', ...rest }: Props) {
  return (
    <button
      type="button"
      {...rest}
      className={`inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-40 ${VARIANT[variant]} ${SIZE[size]} ${className}`}
    >
      {leading}
      {children}
      {trailing}
    </button>
  )
}

/** 视觉稿「下一步 ›」里的那个箭头。 */
export function ChevronRight({ className = '' }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" aria-hidden className={className}>
      <path d="M6 3.5 10.5 8 6 12.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

/** 文字链接（视觉稿「还没有 key？免费申请」是金色的）。 */
export function LinkButton({ children, className = '', ...rest }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type="button"
      {...rest}
      className={`text-md text-amber-text transition-colors duration-150 hover:text-amber-hover ${className}`}
    >
      {children}
    </button>
  )
}
