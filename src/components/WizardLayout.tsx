import type { ReactNode } from 'react'
import { Stepper } from './Stepper'
import type { StepId } from '../state/machine'

/**
 * 向导页骨架（视觉稿第 1、2 张）：左侧步骤条 250px + 右侧内容区。
 * 内容区左右内边距 48px（实测 96px@2x），底部一条分隔线 + 按钮行。
 */
export function WizardLayout({
  title,
  intro,
  children,
  footerLeft,
  footerRight,
  stepSubtitles = {},
}: {
  title: ReactNode
  intro?: ReactNode
  children?: ReactNode
  footerLeft?: ReactNode
  footerRight?: ReactNode
  stepSubtitles?: Partial<Record<StepId, string>>
}) {
  return (
    <div className="flex min-h-0 flex-1">
      <Stepper subtitles={stepSubtitles} />
      <section className="flex min-w-0 flex-1 flex-col bg-window px-wizard">
        <div className="min-h-0 flex-1 overflow-y-auto pt-[34px]">
          <h1 className="text-4xl font-semibold leading-[1.25] text-ink">{title}</h1>
          {intro && <p className="mt-[16px] max-w-[700px] text-md leading-[1.55] text-body">{intro}</p>}
          {children}
          <div className="h-6" />
        </div>
        {(footerLeft || footerRight) && (
          <footer className="flex shrink-0 items-center justify-between border-t border-line pb-[40px] pt-[26px]">
            <div className="flex items-center gap-4">{footerLeft}</div>
            <div className="flex items-center gap-3">{footerRight}</div>
          </footer>
        )}
      </section>
    </div>
  )
}

/** 没有步骤条的整页（设置 / 日志 / 反馈 / 错误页）。 */
export function PlainLayout({
  title,
  right,
  children,
  footer,
}: {
  title?: ReactNode
  right?: ReactNode
  children: ReactNode
  footer?: ReactNode
}) {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-window px-panel">
      {title && (
        <header className="flex shrink-0 items-center justify-between pb-[20px] pt-[30px]">
          <h1 className="text-3xl font-semibold leading-tight text-ink">{title}</h1>
          <div className="flex items-center gap-3">{right}</div>
        </header>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto pb-6">{children}</div>
      {footer && (
        <footer className="flex shrink-0 items-center justify-between border-t border-line pb-[32px] pt-[22px]">
          {footer}
        </footer>
      )}
    </section>
  )
}
