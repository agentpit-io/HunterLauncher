import { StatusDot } from './StatusDot'
import type { ServiceStatus } from '../lib/types'

/**
 * 运行面板里的服务格子（视觉稿第 3 张，3 列 × 2 行）。
 * 左边服务名，中间端口（等宽、暗一档），右边状态点。
 * llm-shim 不发布端口，M0 §5.3 说它的 PublishedPort 是 0 —— 这里显示「内部」而不是 0。
 */
export function ServiceTile({ svc, internalLabel }: { svc: ServiceStatus; internalLabel: string }) {
  return (
    <div className="flex h-[40px] items-center gap-3 rounded-md border border-line bg-card px-4">
      <span className="truncate text-md text-ink-2">{svc.service}</span>
      <span className="tnum ml-auto shrink-0 text-sm text-muted">{svc.port === null ? internalLabel : svc.port}</span>
      <StatusDot health={svc.health} />
    </div>
  )
}
