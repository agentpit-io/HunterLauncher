import type { ReactNode } from 'react'

/** 表单里的一行：小标签 + 控件 + 可选说明。 */
export function Field({
  label,
  hint,
  children,
  className = '',
}: {
  label: ReactNode
  hint?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <div className={className}>
      <div className="mb-[10px] text-sm leading-none text-muted">{label}</div>
      {children}
      {hint && <div className="mt-2 text-xs leading-[1.5] text-muted">{hint}</div>}
    </div>
  )
}

export function TextInput({
  value,
  onChange,
  placeholder,
  mono = false,
  disabled,
}: {
  value: string
  onChange: (v: string) => void
  placeholder?: string
  mono?: boolean
  disabled?: boolean
}) {
  return (
    <input
      type="text"
      spellCheck={false}
      value={value}
      disabled={disabled}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      className={`h-[var(--hl-btn-h)] w-full rounded-md border border-line bg-card px-4 text-md text-ink outline-none transition-colors duration-150 placeholder:text-muted focus:border-amber/70 disabled:opacity-50 ${mono ? 'tnum' : ''}`}
    />
  )
}

export function TextArea({
  value,
  onChange,
  placeholder,
  rows = 5,
}: {
  value: string
  onChange: (v: string) => void
  placeholder?: string
  rows?: number
}) {
  return (
    <textarea
      rows={rows}
      value={value}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      className="w-full resize-none rounded-md border border-line bg-card px-4 py-3 text-md leading-[1.5] text-ink outline-none transition-colors duration-150 placeholder:text-muted focus:border-amber/70"
    />
  )
}

/** 开关。视觉稿里没有，按同一套令牌延展：关=灰蓝轨道，开=琥珀金轨道。 */
export function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={`relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors duration-150 ${on ? 'bg-amber' : 'bg-slate-track border border-line-strong'}`}
    >
      <span
        className={`absolute top-1/2 size-[16px] -translate-y-1/2 rounded-full transition-all duration-150 ${on ? 'left-[19px] bg-on-amber' : 'left-[3px] bg-line-strong'}`}
      />
    </button>
  )
}

/** 一组互斥的小按钮（语言切换、反馈类型都用它）。 */
export function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T
  options: { id: T; label: ReactNode }[]
  onChange: (v: T) => void
}) {
  return (
    <div className="inline-flex gap-2">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          onClick={() => onChange(o.id)}
          className={`h-[34px] rounded-md border px-4 text-md transition-colors duration-150 ${
            value === o.id
              ? 'border-amber/60 bg-amber-soft text-amber-text'
              : 'border-line-strong bg-transparent text-body hover:bg-card hover:text-ink'
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

/** 复选框。 */
export function Checkbox({ on, onChange, children }: { on: boolean; onChange: (v: boolean) => void; children: ReactNode }) {
  return (
    <button type="button" onClick={() => onChange(!on)} className="flex items-start gap-3 text-left">
      <span
        className={`mt-[2px] flex size-[18px] shrink-0 items-center justify-center rounded-[5px] border transition-colors duration-150 ${
          on ? 'border-amber bg-amber' : 'border-line-strong bg-transparent'
        }`}
      >
        {on && (
          <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden>
            <path d="M3.5 8.5 6.5 11.5 12.5 5" fill="none" stroke="var(--hl-on-amber)" strokeWidth="2.1" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        )}
      </span>
      <span className="text-md leading-[1.45] text-body">{children}</span>
    </button>
  )
}
