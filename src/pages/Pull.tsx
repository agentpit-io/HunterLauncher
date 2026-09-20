import { useEffect, useRef, useState } from 'react'
import { Button } from '../components/Button'
import { LogBox } from '../components/LogBox'
import { ProgressBar } from '../components/ProgressBar'
import { WizardLayout } from '../components/WizardLayout'
import * as ipc from '../lib/ipc'
import { bytes, duration, percent } from '../lib/format'
import { useStore } from '../state/context'
import type { ErrorCode } from '../state/machine'
import type { ImagePull, PullProgress } from '../lib/types'

/**
 * 拉取镜像页 —— 视觉稿第 2 张。
 *
 * 数据全部来自 Rust 推过来的 `hunter://pull` 事件（`docker compose --progress json pull`
 * 的逐行解析结果，M0 §5.1）。分母是拉取前从 registry manifest 算好的压缩大小，
 * 所以进度条从第一秒起就是对的。
 *
 * 与视觉稿的取值差异（M0 实测）：
 *   · 镜像源写真实选中的那个（GHCR 或腾讯云 · 香港），不是稿上写死的「阿里云 · 杭州」；
 *   · 总大小是 manifest 实测值（约 849 MB），不是稿上的 2.1 GB —— 那是解压后的磁盘占用；
 *   · 左列显示真实的镜像短名，不是稿上的 hunter-opencode。
 *
 * 「准备」阶段（选源 / 取 compose / 写配置）没有字节进度，这一段显示逐行文字。
 */
export function Pull() {
  const { t, send, locale } = useStore()
  const [p, setP] = useState<PullProgress | null>(null)
  const [lines, setLines] = useState<string[]>([])
  const [err, setErr] = useState<string | null>(null)
  const advanced = useRef(false)

  useEffect(() => {
    let alive = true
    const unlisten: (() => void)[] = []

    void (async () => {
      // 页面可能在事件已经发过之后才挂载，先补一份快照
      try {
        const snap = await ipc.pullProgress()
        if (alive) setP(snap)
      } catch {
        /* 还没开始拉，快照是空的，正常 */
      }
      unlisten.push(await ipc.onPullProgress((next) => alive && setP(next)))
      unlisten.push(
        await ipc.onLogLine((line) => {
          if (alive) setLines((old) => [...old.slice(-80), line])
        }),
      )
      if (alive) {
        try {
          await ipc.startInstall()
        } catch (e) {
          if (alive) setErr(e instanceof Error ? e.message : String(e))
        }
      }
    })()

    return () => {
      alive = false
      unlisten.forEach((f) => f())
    }
    // 只在挂载时跑一次：这一页的生命周期就是一次拉取
  }, [])

  // 拉完自动往下走。配置在拉取**之前**就写好了（compose 要靠 .env 才知道拉哪些镜像），
  // 所以这里 PULL_DONE 与 CONFIG_WRITTEN 一起发，状态机的 WriteConfig 是个瞬时状态。
  useEffect(() => {
    if (p?.phase === 'done' && !advanced.current) {
      advanced.current = true
      send({ type: 'PULL_DONE' })
      send({ type: 'CONFIG_WRITTEN' })
    }
    if (p?.phase === 'failed' && p.error && !advanced.current) {
      advanced.current = true
      // 安装阶段失败的原因不止「拉不下来」一种（项目名冲突、compose 取不到、写配置失败…）。
      // 按 Rust 给的**真实错误码**走；拿不到才退回 PULL_FAILED（I4 修，见 types.ts 的注释）。
      const code = p.errorCode
      if (code && code !== 'E_PULL_FAILED') send({ type: 'FAIL', code: code as ErrorCode, detail: p.error })
      else send({ type: 'PULL_FAILED', detail: p.error })
    }
  }, [p, send])

  const preparing = !p || p.phase === 'preparing' || p.totalBytes === 0
  const logLines = p && p.log.length > 0 ? [...lines, ...p.log].slice(-60) : lines

  return (
    <WizardLayout
      title={t.pull.title}
      intro={p && p.registryLabel ? t.pull.intro(p.registryLabel) : t.pull.preparingIntro}
      footerLeft={
        <span className="text-sm leading-[1.5] text-muted">
          {err ?? (p && p.attempt > 1 ? t.pull.retrying(p.attempt) : t.pull.failHint)}
        </span>
      }
      footerRight={
        <Button
          onClick={() => {
            void ipc.cancelInstall()
            send({ type: 'BACK' })
          }}
        >
          {t.common.cancel}
        </Button>
      }
    >
      <div className="mt-[24px] flex items-end justify-between">
        <div className="flex items-baseline">
          <span className="tnum text-5xl font-medium leading-none text-ink">{preparing ? '—' : p.percent}</span>
          <span className="ml-1 text-md leading-none text-muted">{t.pull.percentUnit}</span>
        </div>
        <div className="tnum text-md leading-none text-dim">
          {preparing ? (
            t.pull.preparing
          ) : (
            <>
              {t.pull.sizeLine(bytes(p.downloadedBytes, 2), bytes(p.totalBytes, 2))}
              {' · '}
              {p.speedBps ? `${bytes(p.speedBps, 1)}/s · ` : ''}
              {p.etaSeconds === null ? t.pull.etaUnknown : t.pull.eta(duration(p.etaSeconds, locale))}
            </>
          )}
        </div>
      </div>

      <div className="mt-[24px] flex flex-col gap-[14px]">
        {(p?.images ?? []).map((img) => (
          <ImageRow key={img.service} img={img} />
        ))}
      </div>

      <LogBox lines={logLines} className="mt-[18px] h-[92px]" autoScroll emptyText={t.pull.preparing} />
    </WizardLayout>
  )
}

function ImageRow({ img }: { img: ImagePull }) {
  const { t } = useStore()
  const done = img.state === 'done'
  const pct = percent(img.downloadedBytes, img.totalBytes)
  const role = (t.pull.roles as Record<string, string>)[img.service] ?? img.service

  return (
    <div className="flex items-start gap-gap">
      <div className="w-[212px] shrink-0">
        <div className="tnum truncate text-sm leading-[1.3] text-ink-2" title={img.ref}>
          {img.shortRef}
        </div>
        <div className="mt-[3px] truncate text-xs leading-[1.3] text-muted">
          {role} · {img.tag} · {img.totalBytes > 0 ? bytes(img.totalBytes, 0) : '—'}
        </div>
      </div>
      <ProgressBar value={pct} done={done} className="mt-[12px] min-w-0 flex-1" />
      <div className="tnum mt-[2px] w-[92px] shrink-0 text-right text-md leading-tight text-dim">
        {done
          ? t.pull.stateDone
          : img.state === 'failed'
            ? t.pull.stateFailed
            : img.state === 'extracting'
              ? t.pull.stateExtracting
              : img.state === 'pending'
                ? t.pull.statePending
                : bytes(img.downloadedBytes, 0)}
      </div>
    </div>
  )
}
