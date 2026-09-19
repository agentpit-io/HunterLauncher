import { Button, ChevronRight } from '../components/Button'
import { Card } from '../components/Card'
import { EnvList } from '../components/EnvList'
import { AlertTriangle, CheckCircle, Spinner } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'

/**
 * Docker 检测页。三种结果对应三套文案：就绪 / 没装 / 装了但 daemon 没起。
 * M0 §5.4 的判定逻辑（进程起不来 = 没装；退出码非 0 且 Server 为 null = daemon 没起）在 Rust 侧，
 * 这一页只负责把结果画出来；拿不到就显示「—」和原因（红线 1）。
 */
export function Docker() {
  const { t, send, state } = useStore()
  const docker = useAsync(() => ipc.detectDocker(), [])
  const d = docker.data

  const ok = d?.installed && d.daemonRunning && d.meetsMinimum
  const missing = d?.installed === false
  const daemonDown = d?.installed === true && d.daemonRunning === false

  const headline = docker.loading
    ? t.docker.detecting
    : missing
      ? t.docker.missingTitle
      : daemonDown
        ? t.docker.daemonDownTitle
        : ok
          ? t.docker.okTitle
          : t.docker.title

  const hint = docker.loading
    ? null
    : missing
      ? t.docker.missingHint
      : daemonDown
        ? t.docker.daemonDownHint
        : ok
          ? t.docker.okHint
          : (docker.error?.message ?? null)

  return (
    <WizardLayout
      title={t.docker.title}
      intro={t.docker.intro}
      footerLeft={
        <Button size="sm" onClick={() => docker.reload()}>
          {t.common.recheck}
        </Button>
      }
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })}>{t.common.back}</Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            disabled={!ok}
            onClick={() => send(state.name === 'CheckDocker' ? { type: 'DOCKER_FOUND' } : { type: 'DAEMON_UP' })}
          >
            {t.common.next}
          </Button>
        </>
      }
    >
      <div className="mt-[30px] flex items-start gap-3">
        {docker.loading ? (
          <Spinner className="mt-[3px] text-amber" />
        ) : ok ? (
          <CheckCircle size={17} className="mt-[2px] text-success" />
        ) : (
          <AlertTriangle className="mt-[2px] text-danger" />
        )}
        <div>
          <div className="text-xl font-medium leading-tight text-ink">{headline}</div>
          {hint && <div className="mt-2 max-w-[640px] text-md leading-[1.5] text-body">{hint}</div>}
        </div>
      </div>

      <Card className="mt-[24px]">
        <EnvList
          items={[
            { label: t.docker.runtime, value: d?.runtimeLabel ?? null, reason: reason(docker.error) },
            { label: t.docker.clientVersion, value: d?.clientVersion ?? null, reason: reason(docker.error) },
            { label: t.docker.serverVersion, value: d?.serverVersion ?? null, reason: reason(docker.error) },
            { label: t.docker.composeVersion, value: d?.composeVersion ?? null, reason: reason(docker.error) },
            { label: t.docker.minVersion, value: t.docker.minVersionValue },
          ]}
        />
      </Card>

      {!ok && !docker.loading && (
        <Card className="mt-gap">
          <ul className="flex flex-col gap-[10px]">
            {[t.docker.installLinux, t.docker.installMac, t.docker.installWindows].map((line) => (
              <li key={line} className="flex gap-2.5 text-sm leading-[1.5] text-body">
                <span className="mt-[7px] size-[5px] shrink-0 rounded-full bg-amber" />
                {line}
              </li>
            ))}
          </ul>
          <div className="mt-[14px] border-t border-line pt-[14px] text-xs leading-[1.5] text-muted">
            {t.docker.licenseNote}
          </div>
        </Card>
      )}
    </WizardLayout>
  )
}

function reason(err: { code: string; message: string } | null): string | undefined {
  return err ? `${err.code} · ${err.message}` : undefined
}
