import { useState } from 'react'
import { Button, ChevronRight } from '../components/Button'
import { Badge } from '../components/Badge'
import { Checkbox, Field, TextInput } from '../components/Field'
import { CheckCircle, Spinner } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import * as ipc from '../lib/ipc'

type Mode = 'gateway' | 'own'

/**
 * 选择模型页：两张卡片（网关 / 自带 key），默认网关。
 *
 * 自带 key 那条路**会真发一次请求**做连通性检查（先试 GET /models，不行再发一次
 * max_tokens=1 的 chat/completions）—— 不做「格式看起来对就算通过」那种假检查（红线 1）。
 * DeepSeek 的 BASE_URL 会自动勾上 LLM_SCHEMA_SANITIZE=1（方案 §5.4）。
 */
export function Model() {
  const { t, send } = useStore()
  const [mode, setMode] = useState<Mode>('gateway')
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [checking, setChecking] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; message: string; sanitize: boolean } | null>(null)

  const ownReady = baseUrl.trim() !== '' && model.trim() !== '' && apiKey.trim() !== ''
  // BASE_URL 一变就重新判一次是不是 DeepSeek（和 Rust 侧同一条规则：只看主机名）
  const autoSanitize = /(^|\.)deepseek\./i.test(hostOf(baseUrl))

  async function next() {
    setChecking(true)
    setResult(null)
    try {
      const r = await ipc.setModel(
        mode === 'own' ? { mode, baseUrl, model, apiKey } : { mode: 'gateway' },
      )
      setResult({ ok: r.ok, message: r.message, sanitize: r.schemaSanitize })
      if (r.ok) {
        send({ type: 'MODEL_CHOSEN' })
        send({ type: 'REGISTRY_CHOSEN' })
      }
    } catch (e) {
      setResult({ ok: false, message: e instanceof Error ? e.message : String(e), sanitize: false })
    } finally {
      setChecking(false)
    }
  }

  return (
    <WizardLayout
      title={t.model.title}
      intro={t.model.intro}
      footerLeft={
        result ? (
          <span className={`max-w-[560px] text-sm leading-[1.5] ${result.ok ? 'text-amber-text' : 'text-danger'}`}>
            {result.message}
          </span>
        ) : undefined
      }
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })}>{t.common.back}</Button>
          <Button
            variant="primary"
            trailing={checking ? undefined : <ChevronRight />}
            leading={checking ? <Spinner size={14} /> : undefined}
            disabled={checking || (mode === 'own' && !ownReady)}
            onClick={() => void next()}
          >
            {checking ? t.model.checking : t.common.next}
          </Button>
        </>
      }
    >
      <div className="mt-[30px] grid grid-cols-2 gap-gap">
        <Choice selected={mode === 'gateway'} onSelect={() => setMode('gateway')}>
          <div className="flex items-center gap-2.5">
            <span className="text-lg font-medium text-ink">{t.model.gatewayTitle}</span>
            <Badge tone="amber">{t.model.gatewayBadge}</Badge>
          </div>
          <p className="mt-[10px] text-sm leading-[1.5] text-body">{t.model.gatewayDesc}</p>
          <ul className="mt-[16px] flex flex-col gap-[9px]">
            {t.model.gatewayPoints.map((p) => (
              <li key={p} className="flex items-start gap-2.5 text-sm leading-[1.45] text-muted">
                <CheckCircle size={13} className="mt-[3px] shrink-0 text-amber" />
                {p}
              </li>
            ))}
          </ul>
        </Choice>

        <Choice selected={mode === 'own'} onSelect={() => setMode('own')}>
          <div className="text-lg font-medium text-ink">{t.model.ownTitle}</div>
          <p className="mt-[10px] text-sm leading-[1.5] text-body">{t.model.ownDesc}</p>
          <div className="mt-[16px] flex flex-col gap-[12px]">
            <Field label={t.model.ownBase}>
              <TextInput value={baseUrl} onChange={setBaseUrl} mono placeholder="https://api.deepseek.com/v1" />
            </Field>
            <Field label={t.model.ownModel}>
              <TextInput value={model} onChange={setModel} mono placeholder="deepseek-chat" />
            </Field>
            <Field label={t.model.ownKey}>
              <TextInput value={apiKey} onChange={setApiKey} mono password placeholder="sk-…" />
            </Field>
            <Checkbox on={autoSanitize} onChange={() => {}} disabled>
              <span className="text-sm">{t.model.ownSanitize}</span>
              <span className="ml-1.5 text-xs text-muted">
                {autoSanitize ? t.model.ownSanitizeAuto : t.model.ownSanitizeHint}
              </span>
            </Checkbox>
          </div>
        </Choice>
      </div>
    </WizardLayout>
  )
}

function hostOf(url: string): string {
  return url.split('://')[1]?.split('/')[0] ?? ''
}

/**
 * 可选中的大卡片。
 * 这里**不能**用 <button>：WebKit 会把 button 的内容盒在垂直方向居中，
 * 卡片被网格拉高之后文字就跑到中间去了（M1 截图对照时发现的）。
 * 改成带 role/tabIndex 的 div，键盘可达性自己补齐。
 */
function Choice({
  selected,
  onSelect,
  children,
}: {
  selected: boolean
  onSelect: () => void
  children: React.ReactNode
}) {
  return (
    <div
      role="radio"
      tabIndex={0}
      aria-checked={selected}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onSelect()
        }
      }}
      className={`cursor-pointer rounded-lg border bg-card p-card text-left transition-colors duration-150 ${
        selected ? 'border-amber/60' : 'border-line hover:border-line-strong'
      }`}
    >
      {children}
    </div>
  )
}
