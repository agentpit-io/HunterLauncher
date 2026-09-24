import { useEffect, useState } from 'react'
import { Button, ChevronRight, LinkButton } from '../components/Button'
import { KeyInput } from '../components/KeyInput'
import { StatCard } from '../components/StatCard'
import { CheckCircle, Spinner } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'
import { isKeyShape } from '../lib/mask'
import { keptKeyExhausted, keptKeyMode } from '../lib/keptkey'
import { thousands } from '../lib/format'
import type { KeptKey, KeyCheckResult } from '../lib/types'

const APPLY_URL = 'https://hunter.agentpit.io/dev/api-keys'

/**
 * 输入 key 页 —— 视觉稿第 1 张。
 *
 * 与视觉稿的取值差异（M0 实测，已记进待办池 P2-1）：
 *   · key 前缀是 hunt_tools_ 不是 hk_；
 *   · 模型别名是 hunter-chat / hunter-deep，不是 hunter-default；
 *   · 第一张卡片视觉稿写的是「套餐 Community」，但网关的 quota 与 models 接口都不返回套餐字段，
 *     按红线 1 不能编，换成 quota 接口真有的「今日剩余」。
 */
export function Key() {
  const { t, send } = useStore()
  const [key, setKey] = useState(ipc.demoSeed?.key ?? '')
  const [result, setResult] = useState<KeyCheckResult | null>(ipc.demoSeed?.keyCheck ?? null)
  const [checking, setChecking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // I14 · F3：上次「只删除应用，保留数据」把 ~/.hunter/app/.env 留下了，
  // 里面就有一把 key。界面上承诺过「重新安装会直接沿用」——
  // 那就别再让人把同一把 key 重新输一遍。Rust 侧只回打码后的样子与校验结论。
  const [kept, setKept] = useState<KeptKey | null>(null)
  const [keptLoading, setKeptLoading] = useState(true)
  const [useKept, setUseKept] = useState(false)

  useEffect(() => {
    let live = true
    void ipc
      .keptKey()
      .then((k) => {
        if (!live) return
        setKept(k)
        // 验得过才自动沿用；验不过就照常让他填，并把原因摆出来（红线 1）
        if (keptKeyMode(k) === 'reuse') setUseKept(true)
      })
      .catch(() => {
        if (live) setKept(null)
      })
      .finally(() => {
        if (live) setKeptLoading(false)
      })
    return () => {
      live = false
    }
  }, [])

  const shapeOk = isKeyShape(key)
  // 沿用那条路上，「已验证」的依据是 Rust 刚问过网关的那一次，不是前端猜的
  const validated = result?.valid === true || (useKept && kept?.check?.valid === true)
  const keptBad = keptKeyMode(kept) === 'bad'

  async function validate() {
    if (!shapeOk) {
      // 本地就判得出来的格式问题不发请求（M0 §1.1：hunt_tools_ 开头、一共 43 位）
      setError(t.key.formatError)
      setResult(null)
      return
    }
    setChecking(true)
    setError(null)
    try {
      const r = await ipc.validateKey(key)
      setResult(r)
      // 文案由 Rust 给：它知道到底是格式不对、网关拒绝、额度用尽还是根本没连上
      // （M0 §1.3 实测网关分不出「填错了」和「已吊销」，所以那两种共用一句话，不编造区分）
      if (!r.valid) setError(r.message ?? t.key.rejected)
    } catch (e) {
      setResult(null)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setChecking(false)
    }
  }

  // 卡片上的数字：沿用那条路用的是 Rust 刚查回来的那一份，没有就是 null（显示「—」）
  const shown = useKept ? (kept?.check ?? null) : result
  const quota = shown?.quota ?? null
  const models = shown?.models ?? []

  return (
    <WizardLayout
      title={t.key.title}
      intro={t.key.intro}
      footerLeft={<LinkButton onClick={() => void ipc.openExternal(APPLY_URL)}>{t.key.applyLink}</LinkButton>}
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })}>{t.common.back}</Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            disabled={!validated}
            onClick={() => {
              send({ type: 'KEY_SUBMIT' })
              send({ type: 'KEY_VALID' })
            }}
          >
            {t.common.next}
          </Button>
        </>
      }
    >
      <div className="mt-[37px]">
        {keptLoading && (
          <div className="mb-[10px] flex items-center gap-2 text-sm leading-none text-muted">
            <Spinner size={14} />
            {t.key.keptChecking}
          </div>
        )}
        {useKept && kept && (
          <div
            data-testid="key-kept"
            className="mb-[14px] rounded-md border border-amber/45 bg-amber-soft px-[14px] py-[12px]"
          >
            <div className="flex items-center gap-2 text-md leading-none text-amber-text">
              <CheckCircle size={15} className="text-amber" />
              {t.key.keptTitle(kept.masked)}
            </div>
            <div className="mt-[8px] text-sm leading-[1.5] text-body">{t.key.keptSub(kept.path)}</div>
            <div className="mt-[8px]">
              <LinkButton
                onClick={() => {
                  setUseKept(false)
                  setResult(null)
                  setError(null)
                }}
              >
                {t.key.keptChange}
              </LinkButton>
            </div>
          </div>
        )}
        {!useKept && keptBad && kept?.check && (
          <div data-testid="key-kept-bad" className="mb-[14px] text-sm leading-[1.5] text-amber-text">
            {t.key.keptBad(kept.check.message ?? t.key.rejected)}
          </div>
        )}
        <div className={useKept ? 'hidden' : undefined}>
        <div className="mb-[10px] text-sm leading-none text-muted">{t.key.label}</div>
        <div className="flex items-center gap-gap">
          <KeyInput
            value={key}
            onChange={(v) => {
              setKey(v)
              setResult(null)
              setError(null)
            }}
            onSubmit={() => void validate()}
            placeholder={t.key.placeholder}
            invalid={!!error}
          />
          {validated ? (
            <div className="flex h-[var(--hl-input-h)] w-[94px] shrink-0 items-center justify-center gap-2 rounded-md border border-amber/45 bg-amber-soft text-md text-amber-text">
              <CheckCircle size={15} className="text-amber" />
              {t.key.validated}
            </div>
          ) : (
            <Button
              variant="secondary"
              className="h-[var(--hl-input-h)] w-[94px] shrink-0"
              disabled={checking || key.length === 0}
              leading={checking ? <Spinner size={14} /> : undefined}
              onClick={() => void validate()}
            >
              {checking ? t.key.validating : t.key.validate}
            </Button>
          )}
        </div>
        {error && <div className="mt-[10px] text-sm leading-none text-danger">{error}</div>}
        {/* key 是好的、但今天额度用完了：不是错误，是**提醒**，所以用琥珀色而不是红色，
            也不挡住「下一步」。I1 之前这条根本不存在 —— `/quota` 在额度用尽时仍然回 200，
            校验直接过，用户要等装完在 Hunter 里对话被拒才知道（见 I1 迭代报告用例 6）。 */}
        {!error && result?.valid && result.reason === 'exhausted' && result.message && (
          <div data-testid="key-exhausted" className="mt-[10px] text-sm leading-[1.5] text-amber-text">
            {result.message}
          </div>
        )}
        </div>
        {/* 沿用那把 key 时额度也可能已经用完 —— 同一句提醒照样要说 */}
        {useKept && keptKeyExhausted(kept) && kept?.check?.message && (
          <div data-testid="key-exhausted" className="mt-[10px] text-sm leading-[1.5] text-amber-text">
            {kept.check.message}
          </div>
        )}
      </div>

      <div className="mt-[23px] grid grid-cols-3 gap-gap">
        <StatCard
          label={t.key.cardRemaining}
          value={quota ? thousands(quota.remaining) : null}
          unit={t.key.cardRemainingUnit}
          sub={quota && quota.rpm !== null && quota.concurrency !== null ? t.key.cardRemainingSub(quota.rpm, quota.concurrency) : undefined}
          reason={t.app.noDataReason}
        />
        <StatCard
          label={t.key.cardQuota}
          value={quota ? thousands(quota.limitDaily) : null}
          unit={t.key.cardQuotaUnit}
          sub={t.key.cardQuotaSub}
          reason={t.app.noDataReason}
        />
        <StatCard
          label={t.key.cardModels}
          value={models.length > 0 ? String(models.length) : null}
          unit={t.key.cardModelsUnit}
          sub={models.map((m) => m.id).join(' · ')}
          reason={t.app.noDataReason}
        />
      </div>

      <p className="mt-[24px] text-md leading-[1.55] text-body">{t.key.advancedHint}</p>
    </WizardLayout>
  )
}
