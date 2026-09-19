import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'
import { Gear } from './Icons'
import type { AppInfo } from '../lib/types'
import type { Dict } from '../i18n'

/**
 * 自绘标题栏。高 44px（视觉稿 89px@2x），底色与窗口一致，底部一条 #181f2d 的细线。
 *
 * 三个圆点的平台行为（总控规则要求「在 Linux/Windows 上的行为要合理」）：
 *   · macOS：不画，用系统原生红绿灯（Rust 侧建窗时用 TitleBarStyle::Overlay + hiddenTitle），
 *     这里只留出 70px 空位，避免内容压在红绿灯下面。
 *   · Linux：自绘三个点，**左红中黄右绿 = 关闭 / 最小化 / 最大化**，和 macOS 的语义一致；
 *     平时是灰蓝的，鼠标移到整组上才显色 —— 视觉稿里它们就是灰的。
 *   · Windows：同样三个点，但整组挪到标题栏**右侧**、顺序反成「最小化 / 最大化 / 关闭」
 *     （待办池 P1-7）。视觉稿是 mac 风格的左上角，照搬到 Windows 上会让用户
 *     每次都点错 —— 关闭按钮的位置是肌肉记忆，不该由视觉稿决定。
 *     **未在 Windows 真机上目视验证**（没有真机），只有代码路径与 CI 编译。
 */
export function TitleBar({ info }: { info: AppInfo | null }) {
  const { t, demo, overlay, setOverlay } = useStore()
  const native = info?.windowChrome === 'native'
  const winLike = info?.platform === 'windows'

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-[var(--hl-titlebar-h)] shrink-0 items-center border-b border-line bg-window pl-[17px] pr-5"
    >
      {/* macOS 要给系统红绿灯留位；Windows 把三个点挪去了右侧，左边不留空 */}
      {native && <div className="w-[70px]" />}
      {!native && !winLike && <Dots t={t} order="mac" />}

      <div data-tauri-drag-region className={`text-base text-label ${winLike ? 'ml-0' : 'ml-[22px]'}`}>
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
        {/* 设置入口。视觉稿里没有画它，但设置页必须有地方进得去 ——
            运行面板右下角那四个按钮是「停止 / 重启 / 日志 / 反馈」，没有设置，
            所以放在标题栏右侧（M1 把这一页做出来了却没有入口，M2 补上）。 */}
        <button
          type="button"
          title={t.common.settings}
          aria-label={t.common.settings}
          onClick={() => setOverlay(overlay === 'settings' ? null : 'settings')}
          className={`flex size-[26px] items-center justify-center rounded-md transition-colors duration-150 hover:bg-card ${
            overlay === 'settings' ? 'text-amber-text' : 'text-faint hover:text-body'
          }`}
        >
          <Gear size={15} />
        </button>
        <div className="tnum text-sm text-faint">{t.app.versionLabel(info?.launcherVersion ?? '—')}</div>
        {winLike && <Dots t={t} order="win" />}
      </div>
    </header>
  )
}

/**
 * 三个圆点。`mac` = 关闭 / 最小化 / 最大化（视觉稿的顺序，放左上角）；
 * `win` = 最小化 / 最大化 / 关闭（Windows 的习惯，放右上角，关闭在最外侧）。
 */
function Dots({ t, order }: { t: Dict; order: 'mac' | 'win' }) {
  const close = (
    <Dot key="close" title={t.common.close} hover="group-hover:bg-[#ff5f57]" onClick={() => void ipc.windowClose()} />
  )
  const min = (
    <Dot key="min" title={t.common.minimize} hover="group-hover:bg-[#febc2e]" onClick={() => void ipc.windowMinimize()} />
  )
  const max = (
    <Dot
      key="max"
      title={t.common.maximize}
      hover="group-hover:bg-[#28c840]"
      onClick={() => void ipc.windowToggleMaximize()}
    />
  )
  return (
    <div className="group flex items-center gap-1.5">{order === 'mac' ? [close, min, max] : [min, max, close]}</div>
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
