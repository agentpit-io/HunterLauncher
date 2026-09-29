import { useState } from 'react'
import { Button, ChevronRight } from '../components/Button'
import { Checkbox } from '../components/Field'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { GRANT_KEYS, grantsFromConfig, hasGrant, toggleGrant, type GrantKey } from '../lib/grants'
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

  /**
   * R5 · F-02：授权到哪一档（三个勾）。
   *
   * **默认值不在这里** —— 它从后端读（`read_settings` 的 `assistGrants`，
   * Rust 侧 `default_grants` 定的是「一级 + 二级，不含三级」）。
   * 界面里另写一份默认值的话，两份迟早会分家。
   *
   * `null` = 用户还没动过，那就照后端当前的值显示。
   */
  const settings = useAsync(() => ipc.readSettings(), [])
  const [picked, setPicked] = useState<GrantKey[] | null>(null)
  const loaded = settings.data !== null
  const current: GrantKey[] = picked ?? (settings.data ? grantsFromConfig(settings.data.assistGrants) : [])

  // 档位写死成 `auto`：这一页不再让用户选档（要改去设置页）
  const MODE: AssistMode = 'auto'

  async function start() {
    setBusy(true)
    setError(null)
    try {
      // 读到了就**显式**把勾的结果送过去；还没读到就整个不传，
      // 由后端按出厂那一档算（见 ipc.assistConsent 的说明）
      await ipc.assistConsent(MODE, allowInstall, loaded ? current : undefined)
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

      {/* R5 · F-02：三级授权。三档是一个**阶梯** —— 勾了三级就含一二级，
          所以后端存的是「上限」，这里显示的也永远是一个前缀（见 lib/grants.ts）。
          默认值来自后端，不是这里写死的。 */}
      <div
        data-testid="consent-grants"
        className="mt-[16px] flex max-w-[860px] flex-col gap-[10px] rounded-lg border border-line bg-card px-5 py-4"
      >
        <div className="text-md font-medium leading-tight text-ink">{t.grants.title}</div>
        <div className="text-xs leading-[1.55] text-muted">{t.grants.hint}</div>
        {GRANT_KEYS.map((k) => (
          <div key={k} className="mt-[2px] flex items-start gap-3">
            <Checkbox
              on={hasGrant(current, k)}
              disabled={busy}
              onChange={(on) => setPicked(toggleGrant(current, k, on))}
            >
              <span className="min-w-0">
                <span className="block text-sm leading-tight text-ink">{t.grants[k].title}</span>
                <span className="mt-[5px] block text-xs leading-[1.5] text-body">
                  {t.grants[k].body}
                </span>
              </span>
            </Checkbox>
          </div>
        ))}
        {current.length === 0 && (
          <div className="text-xs leading-[1.5] text-amber-text">{t.grants.offNote}</div>
        )}
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
