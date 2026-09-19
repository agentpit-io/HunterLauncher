import type { ReactNode } from 'react'

export interface EnvItem {
  label: ReactNode
  value: ReactNode | null
  /** value 为 null 时必须给原因（总控规则红线 1） */
  reason?: string
}

/** 运行面板右侧的环境键值列表：左键右值，行高固定，值用稍亮的颜色。 */
export function EnvList({ items }: { items: EnvItem[] }) {
  return (
    <dl className="flex flex-col">
      {items.map((it, i) => (
        <div key={i} className="flex h-[32px] items-center justify-between gap-4">
          <dt className="shrink-0 text-md text-label">{it.label}</dt>
          <dd
            className={`min-w-0 truncate text-md ${it.value === null ? 'text-muted' : 'text-ink-2'}`}
            title={it.value === null ? it.reason : undefined}
          >
            {it.value === null ? '—' : it.value}
          </dd>
        </div>
      ))}
    </dl>
  )
}
