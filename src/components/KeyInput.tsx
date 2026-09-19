import { useState } from 'react'
import { maskKey } from '../lib/mask'

/**
 * hunter key 输入框（视觉稿第 1 张）。
 *
 * 红线 2：key 只活在这个组件的受控值与内存里。
 *   · 失去焦点后显示打码形态（前缀 + 圆点 + 末 4 位），聚焦时才显示明文，方便用户核对；
 *   · 组件本身不打日志、不往外抛整串 key，只把值交给上层的提交回调。
 */
export function KeyInput({
  value,
  onChange,
  onSubmit,
  placeholder,
  disabled,
  invalid,
}: {
  value: string
  onChange: (v: string) => void
  onSubmit?: () => void
  placeholder?: string
  disabled?: boolean
  invalid?: boolean
}) {
  const [focused, setFocused] = useState(false)
  const showMasked = !focused && value.length > 0

  return (
    <input
      type="text"
      spellCheck={false}
      autoComplete="off"
      autoCorrect="off"
      autoCapitalize="off"
      // 打码态换成只读的展示值；真正的值始终在上层 state 里
      value={showMasked ? maskKey(value) : value}
      onChange={(e) => onChange(e.target.value.trim())}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && onSubmit) onSubmit()
      }}
      placeholder={placeholder}
      disabled={disabled}
      aria-invalid={invalid || undefined}
      className={`tnum h-[var(--hl-input-h)] w-full rounded-md border bg-card px-[22px] text-md text-ink outline-none transition-colors duration-150 placeholder:text-muted disabled:opacity-50 ${
        invalid ? 'border-danger/60 focus:border-danger' : 'border-line focus:border-amber/70'
      }`}
    />
  )
}
