import { CandlestickDecor } from './CandlestickDecor'
import { useStore } from '../state/context'
import { stepperOf, type StepId, type StepStatus } from '../state/machine'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import type { Dict } from '../i18n'

/**
 * 左侧竖向步骤条（视觉稿第 1、2 张）。宽 250px，底色比内容区深一点（#0b0f1a）。
 * 三种点：当前=金色实心、已完成=灰蓝实心、未到=空心描边；点之间用 1px 竖线连起来。
 */
export function Stepper({ subtitles }: { subtitles: Partial<Record<StepId, string>> }) {
  const { state, t } = useStore()
  const steps = stepperOf(state)
  // 视觉稿里 Docker 与「拉取镜像」两步的副标题在**每一页**都显示真实值
  // （「已检测到 …」「约 … · 镜像源」），所以在步骤条这一层取，而不是让每个页面各传一遍。
  // 拿不到就显示「—」并保留原因（总控规则红线 1）。
  const docker = useAsync(() => ipc.detectDocker(), [])
  const pull = useAsync(() => ipc.pullProgress(), [])

  const auto: Partial<Record<StepId, string>> = {}
  if (docker.data?.runtimeLabel) auto.docker = `已检测到 ${docker.data.runtimeLabel}`
  else if (docker.data?.installed === false) auto.docker = '未安装'
  const totalBytes = pull.data?.images.reduce((a, i) => a + i.totalBytes, 0) ?? 0
  auto.pull = t.steps.pull.sub(totalBytes > 0 ? bytes(totalBytes, 0) : t.app.noData, 'GHCR')

  return (
    <aside className="relative flex w-[var(--hl-sidebar-w)] shrink-0 flex-col overflow-hidden border-r border-line bg-sidebar">
      <div className="flex items-center gap-3 px-6 pt-[26px]">
        <Logo />
        <div className="min-w-0">
          <div className="text-lg font-semibold leading-tight text-ink">{t.app.brand}</div>
          <div className="mt-[3px] text-sm leading-tight text-faint">{t.app.slogan}</div>
        </div>
      </div>

      <nav className="mt-[38px] px-6">
        {steps.map((s, i) => (
          <StepRow
            key={s.id}
            status={s.status}
            last={i === steps.length - 1}
            title={t.steps[s.id].title}
            sub={subtitles[s.id] ?? auto[s.id] ?? defaultSub(t, s.id)}
          />
        ))}
      </nav>

      <CandlestickDecor className="absolute bottom-[20px] left-[14px]" />
    </aside>
  )
}

function defaultSub(t: Dict, id: StepId): string {
  const step = t.steps[id]
  return typeof step.sub === 'string' ? step.sub : ''
}

const DOT: Record<StepStatus, string> = {
  current: 'bg-amber border-amber',
  done: 'bg-slate-dot border-slate-dot',
  todo: 'bg-transparent border-line-strong',
}

const TITLE: Record<StepStatus, string> = {
  current: 'text-ink font-medium',
  done: 'text-step-done',
  todo: 'text-faint',
}

function StepRow({ status, title, sub, last }: { status: StepStatus; title: string; sub: string; last: boolean }) {
  return (
    <div className="relative flex gap-[18px] pb-[20px] last:pb-0">
      {/* 竖线挂在整行上（而不是那个 12px 的小列上），才能跨过行的下内边距接到下一个点 */}
      {!last && <span className="absolute left-[6px] top-[19px] bottom-[-5px] w-px bg-line-step" />}
      <div className="flex w-[12px] shrink-0 justify-center">
        <span className={`relative z-10 mt-[5px] size-[12px] shrink-0 rounded-full border ${DOT[status]}`} />
      </div>
      <div className="min-w-0 pb-[2px]">
        <div className={`text-md leading-[1.3] ${TITLE[status]}`}>{title}</div>
        {sub && <div className="mt-[5px] text-xs leading-[1.3] text-muted">{sub}</div>}
      </div>
    </div>
  )
}

/** 视觉稿左上角那个金色圆角方块里的菱形。 */
function Logo() {
  return (
    <div className="flex size-[30px] shrink-0 items-center justify-center rounded-[9px] bg-amber">
      <svg viewBox="0 0 24 24" width="17" height="17" aria-hidden>
        <rect
          x="12"
          y="2.6"
          width="13.3"
          height="13.3"
          rx="2"
          transform="rotate(45 12 2.6)"
          fill="none"
          stroke="var(--hl-on-amber)"
          strokeWidth="1.9"
        />
      </svg>
    </div>
  )
}
