import { Button } from '../components/Button'
import { LogBox } from '../components/LogBox'
import { ProgressBar } from '../components/ProgressBar'
import { WizardLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { bytes, duration, percent } from '../lib/format'
import { useStore } from '../state/context'
import type { ImagePull } from '../lib/types'

/**
 * 拉取镜像页 —— 视觉稿第 2 张。
 *
 * 与视觉稿的取值差异（M0 实测）：
 *   · 镜像源写 GHCR，不是「阿里云 · 杭州」—— M0 §4.2 实测阿里云 ACR 与 Docker Hub 上都没有镜像；
 *   · 总大小 849 MB（= 810.6 MiB，M0 §4.1 实测压缩后字节数），不是视觉稿的 2.1 GB；
 *   · 左列显示 compose 的服务名 + 完整短引用，视觉稿写的 hunter-opencode 不是真实镜像名。
 */
export function Pull() {
  const { t, send, locale } = useStore()
  const pull = useAsync(() => ipc.pullProgress(), [])
  const p = pull.data

  const total = p?.images.reduce((a, i) => a + i.totalBytes, 0) ?? 0
  const doneBytes = p?.images.reduce((a, i) => a + i.downloadedBytes, 0) ?? 0
  const pct = percent(doneBytes, total)

  return (
    <WizardLayout
      title={t.pull.title}
      intro={p ? t.pull.intro(p.registry) : (pull.error?.message ?? t.common.loading)}
      footerLeft={<span className="text-sm leading-[1.5] text-muted">{t.pull.failHint}</span>}
      footerRight={<Button onClick={() => send({ type: 'BACK' })}>{t.common.cancel}</Button>}
    >
      <div className="mt-[24px] flex items-end justify-between">
        <div className="flex items-baseline">
          <span className="tnum text-5xl font-medium leading-none text-ink">{p ? pct : '—'}</span>
          <span className="ml-1 text-md leading-none text-muted">{t.pull.percentUnit}</span>
        </div>
        <div className="tnum text-md leading-none text-dim">
          {p ? (
            <>
              {t.pull.sizeLine(bytes(doneBytes, 2), bytes(total, 2))}
              {' · '}
              {p.etaSeconds === null ? t.pull.etaUnknown : t.pull.eta(duration(p.etaSeconds, locale))}
            </>
          ) : (
            t.app.noData
          )}
        </div>
      </div>

      <div className="mt-[24px] flex flex-col gap-[14px]">
        {(p?.images ?? []).map((img) => (
          <ImageRow key={img.service} img={img} />
        ))}
      </div>

      {p && <LogBox lines={p.log} className="mt-[18px] h-[80px]" autoScroll />}
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
          {role} · {img.tag} · {bytes(img.totalBytes, 0)}
        </div>
      </div>
      <ProgressBar value={pct} done={done} className="mt-[12px] min-w-0 flex-1" />
      <div className="tnum mt-[2px] w-[92px] shrink-0 text-right text-md leading-tight text-dim">
        {done
          ? t.pull.stateDone
          : img.state === 'failed'
            ? t.pull.stateFailed
            : img.state === 'pending'
              ? t.pull.statePending
              : bytes(img.downloadedBytes, 0)}
      </div>
    </div>
  )
}
