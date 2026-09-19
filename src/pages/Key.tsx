import { useState } from 'react'
import { Button, ChevronRight, LinkButton } from '../components/Button'
import { KeyInput } from '../components/KeyInput'
import { StatCard } from '../components/StatCard'
import { CheckCircle, Spinner } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'
import { isKeyShape } from '../lib/mask'
import { thousands } from '../lib/format'
import type { KeyCheckResult } from '../lib/types'

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

  const shapeOk = isKeyShape(key)
  const validated = result?.valid === true

  async function validate() {
    if (!shapeOk) {
      setError(t.key.formatError)
      setResult(null)
      return
    }
    setChecking(true)
    setError(null)
    try {
      const r = await ipc.validateKey(key)
      setResult(r)
      if (!r.valid) setError(t.key.rejected)
    } catch (e) {
      setResult(null)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setChecking(false)
    }
  }

  const quota = result?.quota ?? null
  const models = result?.models ?? []

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
