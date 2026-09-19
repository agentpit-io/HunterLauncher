import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'
import type { AppInfo } from '../lib/types'

/**
 * 自绘标题栏。高 44px（视觉稿 89px@2x），底色与窗口一致，底部一条 #181f2d 的细线。
 *
 * 三个圆点的平台行为（总控规则要求「在 Linux/Windows 上的行为要合理」）：
 *   · macOS：不画，用系统原生红绿灯（Rust 侧建窗时用 TitleBarStyle::Overlay + hiddenTitle），
 *     这里只留出 70px 空位，避免内容压在红绿灯下面。
 *   · Linux / Windows：自绘三个点，**左红中黄右绿 = 关闭 / 最小化 / 最大化**，
 *     和 macOS 的语义一致；平时是灰蓝的，鼠标移到整组上才显色 —— 视觉稿里它们就是灰的。
 *     顺序保持在左侧，与视觉稿一致（Windows 用户会觉得位置怪，但这是视觉稿定的，
 *     已在 M1 报告里记为「按稿实现」）。
 */
export function TitleBar({ info }: { info: AppInfo | null }) {
  const { t, demo } = useStore()
  const native = info?.windowChrome === 'native'

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-[var(--hl-titlebar-h)] shrink-0 items-center border-b border-line bg-window pl-[17px] pr-5"
    >
      {native ? (
        <div className="w-[70px]" />
      ) : (
        <div className="group flex items-center gap-1.5">
          <Dot title={t.common.close} hover="group-hover:bg-[#ff5f57]" onClick={() => void ipc.windowClose()} />
          <Dot title="—" hover="group-hover:bg-[#febc2e]" onClick={() => void ipc.windowMinimize()} />
          <Dot title="⤢" hover="group-hover:bg-[#28c840]" onClick={() => void ipc.windowToggleMaximize()} />
        </div>
      )}

      <div data-tauri-drag-region className="ml-[22px] text-base text-label">
        {t.app.windowTitle}
      </div>

      <div className="ml-auto flex items-center gap-3">
        {demo && (
          <span
            title={t.app.demoTip}
            className="rounded-sm border border-amber/45 bg-amber-soft px-2 py-[3px] text-xs leading-none text-amber-text"
          >
            {t.app.demoBadge}
          </span>
        )}
        <div className="tnum text-sm text-faint">{t.app.versionLabel(info?.launcherVersion ?? '—')}</div>
      </div>
    </header>
  )
}

function Dot({ title, hover, onClick }: { title: string; hover: string; onClick: () => void }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={`size-[12px] rounded-full bg-[#1c2b40] transition-colors duration-150 ${hover}`}
    />
  )
}
