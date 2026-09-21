import { useState } from 'react'
import { Button, ChevronRight } from '../components/Button'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'
import type { AssistMode } from '../lib/types'

/**
 * 一次授权页（I5 · AI 自动驾驶安装方案 §三；I8 改成全自动）。
 *
 * 这一页是整轮改动的**承诺书**：上面写的每一条「绝不会做」，在 Rust 侧
 * `assist/guard.rs` 里都有一条对应的硬校验，并且各有一条「模型真的提出来了、
 * 被拒绝了」的单元测试。文案与守卫必须一起改 —— 这里写了却没拦住，就是骗人。
 *
 * ## I8：这一页只剩一个按钮
 *
 * I5–I7 这里有三个出口：「授权 AI 自动安装」「我想每一步自己确认」「不用 AI」。
 * 用户 2026-09-21 22:35 的决定是**不要让用户参与决策**，于是档位选择整个挪到了
 * 设置页，这一页回到它本来的职责：**说清楚会做什么、绝不做什么，然后开始**。
 *
 * 用户在整条主路径上的点击次数因此变成：输 key 一次 + 这里一次 = **两次**，
 * 此后到六个服务健康为止一次都不用点（验收口径见 I8 迭代报告）。
 */
export function Consent() {
  const { t, send } = useStore()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // I7：默认勾着。I8 起它覆盖整条兜底链（内置运行时 / OrbStack / Homebrew）。
  const [allowInstall, setAllowInstall] = useState(true)

  // 档位写死成 `auto`：这一页不再让用户选档（要改去设置页）
  const MODE: AssistMode = 'auto'

  async function start() {
    setBusy(true)
    setError(null)
    try {
      await ipc.assistConsent(MODE, allowInstall)
      send({ type: 'CONSENT_GIVEN' })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  return (
    <WizardLayout
      title={t.consent.title}
      intro={t.consent.intro}
      footerLeft={
        /* 选模型是支线：默认走 Hunter 网关，点了这一条才去那一页。
           主路径上「授权 → 自动安装」中间不再有任何一次点击。
           I8：「不用 AI」那一档跟着别的档位一起挪去了设置页 —— 这一页不再让用户选档。 */
        <button
          type="button"
          className="text-sm text-muted underline-offset-4 hover:text-body hover:underline"
          onClick={() => send({ type: 'CHOOSE_MODEL' })}
          disabled={busy}
          data-testid="consent-own-model"
        >
          {t.consent.ownModel}
        </button>
      }
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })} disabled={busy}>
            {t.common.back}
          </Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            data-testid="consent-start"
            disabled={busy}
            onClick={() => void start()}
          >
            {busy ? t.common.working : t.consent.startBtn}
          </Button>
        </>
      }
    >
      <div className="mt-[28px] grid max-w-[860px] grid-cols-2 gap-gap">
        <Panel tone="will" title={t.consent.willTitle} items={t.consent.will} />
        <Panel tone="never" title={t.consent.neverTitle} items={t.consent.never} />
      </div>

      <div className="mt-[16px] max-w-[860px] rounded-lg border border-amber/35 bg-amber-soft px-5 py-4">
        <div className="text-md font-medium leading-tight text-amber-text">{t.consent.askTitle}</div>
        <ul className="mt-[10px] flex flex-col gap-[6px]">
          {t.consent.ask.map((x) => (
            <li key={x} className="text-sm leading-[1.5] text-body">
              · {x}
            </li>
          ))}
        </ul>
      </div>

      {/* I7 · 那一项默认勾选的勾。勾着 = 「电脑上没有 Docker 时允许 AI 装一套」。
          文案里把**装在哪、改不改系统、怎么卸**三件事都写清楚 ——
          默认勾选的前提是用户看一眼就知道自己同意了什么。 */}
      <label
        data-testid="consent-allow-install"
        className="mt-[16px] flex max-w-[860px] cursor-pointer items-start gap-[10px] rounded-lg border border-line bg-card px-5 py-4"
      >
        <input
          type="checkbox"
          className="mt-[3px] size-[15px] shrink-0 accent-amber"
          checked={allowInstall}
          disabled={busy}
          onChange={(e) => setAllowInstall(e.target.checked)}
        />
        <span className="min-w-0">
          <span className="block text-md leading-tight text-ink">{t.consent.allowInstallTitle}</span>
          <span className="mt-[6px] block text-sm leading-[1.5] text-body">{t.consent.allowInstallBody}</span>
          {!allowInstall && (
            <span className="mt-[6px] block text-sm leading-[1.5] text-muted">{t.consent.allowInstallOff}</span>
          )}
        </span>
      </label>

      <p className="mt-[16px] max-w-[860px] text-sm leading-[1.55] text-muted">{t.consent.footnote}</p>
      {error && <div className="mt-[10px] text-sm text-danger">{error}</div>}
    </WizardLayout>
  )
}

function Panel({ tone, title, items }: { tone: 'will' | 'never'; title: string; items: string[] }) {
  const never = tone === 'never'
  return (
    <div className="rounded-lg border border-line bg-card px-5 py-4">
      <div className={`text-md font-medium leading-tight ${never ? 'text-danger' : 'text-ink'}`}>{title}</div>
      <ul className="mt-[12px] flex flex-col gap-[8px]">
        {items.map((x) => (
          <li key={x} className="flex gap-[8px] text-sm leading-[1.5] text-body">
            <span className={`shrink-0 ${never ? 'text-danger' : 'text-success'}`}>{never ? '✕' : '✓'}</span>
            <span>{x}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}
