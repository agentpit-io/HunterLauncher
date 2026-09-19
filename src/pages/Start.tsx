import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { ProgressBar } from '../components/ProgressBar'
import { StatusDot } from '../components/StatusDot'
import { WizardLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { percent } from '../lib/format'
import { useStore } from '../state/context'
import type { Health } from '../lib/types'

/**
 * 启动页：compose up -d 之后轮询 6 个服务的健康状态。
 * M0 §3.1 实测：只有 postgres / redis 的 healthcheck 写在 compose 里，另外四个烧在镜像里，
 * 所以健康状态只能读 `docker compose ps --format json` 的 Health 字段，不能去 compose 文件里推。
 */
export function Start() {
  const { t, send } = useStore()
  const rt = useAsync(() => ipc.startingStatus(), [])
  const services = rt.data?.services ?? []
  const okCount = services.filter((s) => s.health === 'healthy').length
  const total = services.length || 6

  return (
    <WizardLayout
      title={t.start.title}
      intro={t.start.intro}
      footerLeft={<span className="text-sm leading-[1.5] text-muted">{t.start.timeoutHint}</span>}
      footerRight={
        <Button variant="primary" disabled={okCount < total} onClick={() => send({ type: 'START_OK' })}>
          {t.common.next}
        </Button>
      }
    >
      <div className="mt-[30px] flex items-end justify-between">
        <div className="tnum text-5xl font-medium leading-none text-ink">
          {rt.data ? `${okCount} / ${total}` : '—'}
        </div>
        <div className="text-md leading-none text-dim">{t.start.healthy(okCount, total)}</div>
      </div>
      <ProgressBar value={percent(okCount, total)} className="mt-[18px]" />

      <div className="mt-[26px] grid grid-cols-2 gap-gap">
        {services.length === 0 && !rt.loading && (
          <Card className="col-span-2 text-md text-muted">
            {t.app.noData} · {rt.error?.message ?? t.app.noDataReason}
          </Card>
        )}
        {services.map((s) => (
          <Card key={s.service} className="flex items-center gap-3 !py-[14px]">
            <StatusDot health={s.health} pulse={s.health === 'starting'} />
            <span className="text-md text-ink-2">{s.service}</span>
            <span className="tnum ml-auto text-sm text-muted">{s.port === null ? t.common.internal : s.port}</span>
            <span className={`text-sm ${s.health === 'healthy' ? 'text-dim' : 'text-muted'}`}>
              {(t.start.states as Record<Health, string>)[s.health]}
            </span>
          </Card>
        ))}
      </div>
    </WizardLayout>
  )
}
