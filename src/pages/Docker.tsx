import { useState } from 'react'
import { AssistPanel } from '../components/AssistPanel'
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
 *
 * 判定全在 Rust 侧（M0 §5.4：进程起不来 = 没装；退出码非 0 且 Server 为 null = daemon 没起），
 * 这一页只负责把结果画出来；拿不到就显示「—」和原因（红线 1）。
 * 安装引导的文案与链接也由 Rust 按平台给出 —— 只有它知道现在跑在哪个系统上。
 */
export function Docker() {
  const { t, send, state } = useStore()
  const docker = useAsync(() => ipc.detectDocker(), [])
  const [starting, setStarting] = useState(false)
  const [startMsg, setStartMsg] = useState<string | null>(null)
  const d = docker.data

  const ok = !!d?.installed && d.daemonRunning && d.meetsMinimum
  const missing = d?.installed === false
  const daemonDown = d?.installed === true && d.daemonRunning === false
  const code = missing ? 'E_DOCKER_MISSING' : daemonDown ? 'E_DAEMON_DOWN' : d && !d.meetsMinimum ? 'E_DOCKER_MISSING' : null

  const headline = docker.loading
    ? t.docker.detecting
    : missing
      ? t.docker.missingTitle
      : daemonDown
        ? t.docker.daemonDownTitle
        : ok
          ? t.docker.okTitle
          : t.docker.title

  // 优先显示 Rust 给出的**具体**原因（它拿到了 docker 的原话），没有才退回通用文案
  const hint = docker.loading
    ? null
    : (d?.problem ?? (ok ? t.docker.okHint : (docker.error?.message ?? null)))

  async function tryStart() {
    setStarting(true)
    setStartMsg(null)
    try {
      setStartMsg(await ipc.startDaemon())
    } catch (e) {
      setStartMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setStarting(false)
      docker.reload()
    }
  }

  return (
    <WizardLayout
      title={t.docker.title}
      intro={t.docker.intro}
      footerLeft={
        <div className="flex items-center gap-[10px]">
          <Button size="sm" disabled={docker.loading} onClick={() => docker.reload()}>
            {t.common.recheck}
          </Button>
          {daemonDown && (
            <Button size="sm" disabled={starting} leading={starting ? <Spinner size={13} /> : undefined} onClick={() => void tryStart()}>
              {t.docker.tryStart}
            </Button>
          )}
          {code && <span className="tnum text-xs text-muted">{code}</span>}
        </div>
      }
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })}>{t.common.back}</Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            disabled={!ok}
            onClick={() => {
              // 一次检测同时得出「装没装」与「daemon 起没起」两个结论，所以两个事件一起发。
              // 状态机里 CheckDocker 与 CheckDaemon 渲染的是**同一页**，只发一个事件的话
              // 用户会看到点了「下一步」页面纹丝不动，得再点一次（M2 用 xdotool 走真实流程时撞出来的）。
              if (state.name === 'CheckDocker') send({ type: 'DOCKER_FOUND' })
              send({ type: 'DAEMON_UP' })
            }}
          >
            {t.common.next}
          </Button>
        </>
      }
    >
      <div className="mt-[26px] flex items-start gap-3">
        {docker.loading ? (
          <Spinner className="mt-[3px] text-amber" />
        ) : ok ? (
          <CheckCircle size={17} className="mt-[2px] text-success" />
        ) : (
          <AlertTriangle className="mt-[2px] text-danger" />
        )}
        <div className="min-w-0">
          <div className="text-xl font-medium leading-tight text-ink">{headline}</div>
          {hint && <div className="mt-2 max-w-[660px] text-md leading-[1.5] text-body">{hint}</div>}
          {startMsg && <div className="mt-2 max-w-[660px] text-sm leading-[1.5] text-amber-text">{startMsg}</div>}
        </div>
      </div>

      <Card className="mt-[20px]">
        <EnvList
          items={[
            { label: t.docker.runtime, value: d?.runtimeLabel ?? null, reason: reason(docker.error) },
            { label: t.docker.clientVersion, value: d?.clientVersion ?? null, reason: reason(docker.error) },
            { label: t.docker.serverVersion, value: d?.serverVersion ?? null, reason: reason(docker.error) },
            { label: t.docker.composeVersion, value: d?.composeMode ?? d?.composeVersion ?? null, reason: reason(docker.error) },
            // I4：把「启动器实际用的是哪一个 docker」摆到明面上。
            // mac 上同时装过 Docker Desktop 与 OrbStack 是常事，不写出来谁也说不清。
            {
              label: t.docker.dockerPath,
              value: d?.dockerPath ? <PathValue path={d.dockerPath} source={d.dockerPathSource} link={d.dockerLinkTarget} /> : null,
              reason: d ? t.docker.dockerPathNone(d.dockerProbe?.length ?? 0) : reason(docker.error),
            },
            ...(d?.wsl === null || d?.wsl === undefined
              ? []
              : [{ label: 'WSL2', value: d.wsl ? t.common.yes : t.common.no }]),
            { label: t.docker.minVersion, value: t.docker.minVersionValue },
          ]}
        />
      </Card>

      {/* 诊断助手（I4）。Docker 这一步是最容易卡住的一步，也是那个 macOS P0 的现场。
          规则层零 token，认不出来才会给出「让 AI 帮我看看」。 */}
      {!ok && !docker.loading && <AssistPanel className="mt-gap" errorCode={code ?? undefined} errorMessage={d?.problem ?? undefined} stage="docker" />}

      {/* 安装引导：内容来自 Rust 的 install_guide（按平台生成），不是前端写死的 */}
      {!ok && !docker.loading && d?.installGuide && (
        <Card className="mt-gap max-h-[224px] overflow-y-auto">
          <div className="text-md font-medium text-ink">{d.installGuide.title}</div>
          <ul className="mt-[12px] flex flex-col gap-[10px]">
            {d.installGuide.steps.map((s, i) => (
              <li key={i} className="flex gap-2.5 text-sm leading-[1.5] text-body">
                <span className="mt-[7px] size-[5px] shrink-0 rounded-full bg-amber" />
                <span className="min-w-0">
                  {s.text}
                  {s.url && (
                    <button
                      type="button"
                      className="ml-1.5 cursor-pointer text-amber-text underline-offset-2 hover:underline"
                      onClick={() => void ipc.openExternal(s.url!)}
                    >
                      {t.docker.openLink}
                    </button>
                  )}
                </span>
              </li>
            ))}
          </ul>
          <div className="mt-[14px] border-t border-line pt-[14px] text-xs leading-[1.5] text-muted">
            {d.installGuide.licenseNote}
          </div>
        </Card>
      )}
    </WizardLayout>
  )
}

function reason(err: { code: string; message: string } | null): string | undefined {
  return err ? `${err.code} · ${err.message}` : undefined
}

/** 「/usr/local/bin/docker（已知位置 → …/OrbStack.app/…/xbin/docker）」 */
function PathValue({ path, source, link }: { path: string; source: string | null; link: string | null }) {
  const title = link ? `${path}（${source ?? '?'} · 软链指向 ${link}）` : `${path}（${source ?? '?'}）`
  return (
    <span className="tnum" title={title}>
      {path}
      {source && <span className="ml-1.5 text-muted">{source}</span>}
    </span>
  )
}
