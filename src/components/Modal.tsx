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
  wide = false,
}: {
  title: ReactNode
  children: ReactNode
  footer: ReactNode
  /** 传了就允许 Esc / 点遮罩关闭；退出询问这种必须做选择的就不传 */
  onClose?: () => void
  testId?: string
  /** 内容密的那几个用宽一档（I16 的上传预览：一屏要摆下 meta + 整段正文） */
  wide?: boolean
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
      {/* **标题与按钮永远看得见，中间那段自己滚**（I16 修的）。
          原来整张卡片没有高度上限：上传预览那一屏内容一多，标题被顶出屏幕上沿、
          「确认上传」被顶出下沿 —— 用户既看不到这是在问什么，也点不到按钮。
          截图那一关抓到的（`docs/screenshots/I16/i16-upload-preview.png` 的第一版）。 */}
      <div
        className={`flex max-h-[calc(100vh-56px)] w-full flex-col rounded-lg border border-line-strong bg-card p-6 shadow-2xl ${
          wide ? 'max-w-[760px]' : 'max-w-[520px]'
        }`}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="shrink-0 text-lg font-semibold leading-none text-ink">{title}</div>
        <div className="mt-[16px] min-h-0 flex-1 overflow-y-auto text-md leading-[1.6] text-body">
          {children}
        </div>
        <div className="mt-[24px] flex shrink-0 flex-wrap justify-end gap-[10px]">{footer}</div>
      </div>
    </div>
  )
}
