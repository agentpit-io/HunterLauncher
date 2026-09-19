import { useState } from 'react'
import { Button, ChevronRight } from '../components/Button'
import { Badge } from '../components/Badge'
import { Checkbox, Field, TextInput } from '../components/Field'
import { CheckCircle } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'

type Mode = 'gateway' | 'own'

/** 选择模型页：两张卡片（网关 / 自带 key），默认网关。 */
export function Model() {
  const { t, send } = useStore()
  const [mode, setMode] = useState<Mode>('gateway')
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [sanitize, setSanitize] = useState(false)

  const ownReady = baseUrl.trim() !== '' && model.trim() !== '' && apiKey.trim() !== ''

  return (
    <WizardLayout
      title={t.model.title}
      intro={t.model.intro}
      footerRight={
        <>
          <Button onClick={() => send({ type: 'BACK' })}>{t.common.back}</Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            disabled={mode === 'own' && !ownReady}
            onClick={() => {
              send({ type: 'MODEL_CHOSEN' })
              send({ type: 'REGISTRY_CHOSEN' })
            }}
          >
            {t.common.next}
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
              <TextInput value={apiKey} onChange={setApiKey} mono placeholder="sk-…" />
            </Field>
            <Checkbox on={sanitize} onChange={setSanitize}>
              <span className="text-sm">{t.model.ownSanitize}</span>
              <span className="ml-1.5 text-xs text-muted">{t.model.ownSanitizeHint}</span>
            </Checkbox>
          </div>
        </Choice>
      </div>
    </WizardLayout>
  )
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
