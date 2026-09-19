import type { ReactNode } from 'react'
import { useEffect } from 'react'

/**
 * 模态框。视觉稿里没有这个元素，按同一套设计令牌延展：
 * 半透明黑遮罩 + 卡片底（#0f1626）+ 1px 描边 + 12px 圆角，和别处的卡片一致。
 *
 * 做成**界面里的弹窗**而不是系统对话框是有意的：系统对话框在 Xvfb 里既截不了图、
 * 也点不动，退出询问那两条分支就没法真验（总控规则「前端完成的标准」）。
 */
export function Modal({
  title,
  children,
  footer,
  onClose,
  testId,
}: {
  title: ReactNode
  children: ReactNode
  footer: ReactNode
  /** 传了就允许 Esc / 点遮罩关闭；退出询问这种必须做选择的就不传 */
  onClose?: () => void
  testId?: string
}) {
  useEffect(() => {
    if (!onClose) return
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 px-8"
      data-testid={testId}
      onClick={onClose ? () => onClose() : undefined}
    >
      <div
        className="w-full max-w-[520px] rounded-lg border border-line-strong bg-card p-6 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="text-lg font-semibold leading-none text-ink">{title}</div>
        <div className="mt-[16px] text-md leading-[1.6] text-body">{children}</div>
        <div className="mt-[24px] flex flex-wrap justify-end gap-[10px]">{footer}</div>
      </div>
    </div>
  )
}
