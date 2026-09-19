import { useEffect, useRef, useState } from 'react'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { ProgressBar } from '../components/ProgressBar'
import { StatusDot } from '../components/StatusDot'
import { WizardLayout } from '../components/WizardLayout'
import * as ipc from '../lib/ipc'
import { percent } from '../lib/format'
import { useStore } from '../state/context'
import type { Health, ServiceStatus } from '../lib/types'

const EXPECTED = ['postgres', 'redis', 'llm-shim', 'api', 'opencode', 'web']

/**
 * 启动页：`compose up -d` 之后轮询 6 个服务的健康状态，超时 180 秒。
 *
 * M0 §3.1 实测：只有 postgres / redis 的 healthcheck 写在 compose 里，另外四个烧在镜像里，
 * 所以健康状态只能读 `docker compose ps --format json` 的 Health 字段，不能去 compose 文件里推。
 * 超时时 Rust 会指出**具体是哪个服务**没就绪（方案 §18 对 E_START_TIMEOUT 的要求）。
 */
export function Start() {
  const { t, send } = useStore()
  const [services, setServices] = useState<ServiceStatus[]>([])
  const [note, setNote] = useState<string | null>(null)
  const [elapsed, setElapsed] = useState(0)
  const done = useRef(false)

  useEffect(() => {
    let alive = true
    const unlisten: (() => void)[] = []
    const t0 = Date.now()

    void (async () => {
      unlisten.push(await ipc.onStartProgress((v) => alive && setServices(v)))
      unlisten.push(await ipc.onLogLine((line) => alive && setNote(line)))
      try {
        await ipc.startStack()
      } catch (e) {
        if (alive) setNote(e instanceof Error ? e.message : String(e))
      }
    })()

    // 事件之外再轮询一份：页面挂载晚了或者事件漏了也不会卡住
    const timer = window.setInterval(() => {
      setElapsed(Math.round((Date.now() - t0) / 1000))
      void ipc
        .startingStatus()
        .then((s) => {
          if (!alive) return
          if (s.services.length > 0) setServices(s.services)
          const last = s.log[s.log.length - 1]
          if (last && last.includes('没就绪')) setNote(last)
        })
        .catch(() => {})
    }, 2000)

    return () => {
      alive = false
      window.clearInterval(timer)
      unlisten.forEach((f) => f())
    }
  }, [])

  const ready = services.filter((s) => s.health === 'healthy' || (s.health === 'none' && s.state === 'running'))
  const okCount = ready.length
  const total = Math.max(services.length, EXPECTED.length)

  useEffect(() => {
    if (okCount >= EXPECTED.length && !done.current) {
      done.current = true
      send({ type: 'START_OK' })
    }
  }, [okCount, send])

  // 超过 180 秒还没齐就报超时，文案里带上是哪个服务卡住了
  useEffect(() => {
    if (elapsed > 185 && !done.current) {
      done.current = true
      const stuck = EXPECTED.filter((e) => !ready.some((s) => s.service === e)).join('、')
      send({ type: 'START_TIMEOUT', detail: note ?? t.start.stuck(stuck) })
    }
  }, [elapsed, ready, note, send, t])

  return (
    <WizardLayout
      title={t.start.title}
      intro={t.start.intro}
      footerLeft={
        <span className="max-w-[620px] text-sm leading-[1.5] text-muted">
          {note ?? t.start.timeoutHint}
        </span>
      }
      footerRight={
        <Button variant="primary" disabled={okCount < EXPECTED.length} onClick={() => send({ type: 'START_OK' })}>
          {t.common.next}
        </Button>
      }
    >
      <div className="mt-[30px] flex items-end justify-between">
        <div className="tnum text-5xl font-medium leading-none text-ink">
          {services.length > 0 ? `${okCount} / ${total}` : '—'}
        </div>
        <div className="tnum text-md leading-none text-dim">
          {t.start.healthy(okCount, total)}
          {elapsed > 0 && ` · ${t.start.elapsed(elapsed)}`}
        </div>
      </div>
      <ProgressBar value={percent(okCount, total)} className="mt-[18px]" />

      <div className="mt-[26px] grid grid-cols-2 gap-gap">
        {services.length === 0 && (
          <Card className="col-span-2 text-md text-muted">{t.start.waiting}</Card>
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
