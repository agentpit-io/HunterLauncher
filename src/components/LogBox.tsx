import { forwardRef, useEffect, useImperativeHandle, useRef } from 'react'

/**
 * 日志框。视觉稿里它比卡片更深（#090d16），等宽字体，13px。
 * 允许选中复制，所以带 .selectable。
 */
export const LogBox = forwardRef<
  HTMLDivElement,
  {
    lines: string[]
    className?: string
    emptyText?: string
    autoScroll?: boolean
    /** 需要高亮成金色的片段（视觉稿第 3 张里 "300,000 tokens" 是金色的） */
    highlight?: RegExp
  }
>(function LogBox({ lines, className = '', emptyText = '—', autoScroll = false, highlight }, outer) {
  const ref = useRef<HTMLDivElement>(null)
  // 日志页要自己判断「用户是不是已经滚到底了」，所以把内部的 DOM 节点透出去
  useImperativeHandle(outer, () => ref.current as HTMLDivElement, [])
  useEffect(() => {
    if (autoScroll && ref.current) ref.current.scrollTop = ref.current.scrollHeight
  }, [lines, autoScroll])

  return (
    <div
      ref={ref}
      className={`selectable overflow-auto rounded-lg border border-line bg-log px-card py-4 ${className}`}
    >
      {lines.length === 0 ? (
        <div className="tnum text-sm text-muted">{emptyText}</div>
      ) : (
        <div className="tnum flex flex-col gap-[7px] text-sm leading-[1.25] text-dim">
          {lines.map((line, i) => (
            <div key={i} className="whitespace-pre-wrap break-all">
              {highlight ? renderHighlighted(line, highlight) : line}
            </div>
          ))}
        </div>
      )}
    </div>
  )
})

function renderHighlighted(line: string, re: RegExp) {
  const parts: React.ReactNode[] = []
  const rx = new RegExp(re.source, re.flags.includes('g') ? re.flags : `${re.flags}g`)
  let last = 0
  let m: RegExpExecArray | null
  while ((m = rx.exec(line)) !== null) {
    if (m.index > last) parts.push(line.slice(last, m.index))
    parts.push(
      <span key={`${m.index}-${m[0]}`} className="text-amber">
        {m[0]}
      </span>,
    )
    last = m.index + m[0].length
    if (m[0].length === 0) rx.lastIndex += 1
  }
  if (last < line.length) parts.push(line.slice(last))
  return parts
}
