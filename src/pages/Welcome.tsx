import { Button, ChevronRight } from '../components/Button'
import { Card } from '../components/Card'
import { Checkbox, Field, SegmentedControl } from '../components/Field'
import { AlertTriangle, CheckCircle, Spinner } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import { LOCALES, type Locale } from '../i18n'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import type { HwReport } from '../lib/types'
import { useState } from 'react'

export function Welcome() {
  const { t, locale, setLocale, send } = useStore()
  const [agreed, setAgreed] = useState(false)
  const hw = useAsync(() => ipc.hardwareCheck(), [])

  return (
    <WizardLayout
      title={t.welcome.title}
      intro={t.welcome.intro}
      footerRight={
        <Button variant="primary" disabled={!agreed} trailing={<ChevronRight />} onClick={() => send({ type: 'ACCEPT_TERMS' })}>
          {t.welcome.start}
        </Button>
      }
    >
      <div className="mt-[28px] flex flex-col gap-[22px]">
        <Field label={t.welcome.languageLabel}>
          <SegmentedControl<Locale>
            value={locale}
            onChange={setLocale}
            options={LOCALES.map((l) => ({ id: l.id, label: l.label }))}
          />
        </Field>

        <HardwareCheckCard
          loading={hw.loading}
          report={hw.data}
          onRecheck={() => hw.reload()}
        />

        <Card>
          <div className="text-md font-medium text-ink">{t.welcome.privacyTitle}</div>
          <div className="mt-[14px] grid grid-cols-2 gap-x-gap gap-y-[10px]">
            <ul className="flex flex-col gap-[10px]">
              {t.welcome.privacyDo.map((line) => (
                <li key={line} className="flex gap-2.5 text-sm leading-[1.5] text-body">
                  <span className="mt-[7px] size-[5px] shrink-0 rounded-full bg-amber" />
                  {line}
                </li>
              ))}
            </ul>
            <ul className="flex flex-col gap-[10px]">
              {t.welcome.privacyDont.map((line) => (
                <li key={line} className="flex gap-2.5 text-sm leading-[1.5] text-muted">
                  <span className="mt-[7px] size-[5px] shrink-0 rounded-full bg-line-strong" />
                  {line}
                </li>
              ))}
            </ul>
          </div>
        </Card>

        <Field label={t.welcome.termsLabel}>
          <Checkbox on={agreed} onChange={setAgreed}>
            {t.welcome.termsText}
            <span className="text-amber-text"> {t.welcome.termsLink}</span>
          </Checkbox>
        </Field>
      </div>
    </WizardLayout>
  )
}

function itemVal(report: HwReport, id: string): string {
  return report.items.find((i) => i.id === id)?.value ?? '—'
}

export function HardwareCheckCard({
  loading,
  report,
  onRecheck,
}: {
  loading: boolean
  report: HwReport | null
  onRecheck: () => void
}) {
  const { t } = useStore()

  if (loading && !report) {
    return (
      <Card>
        <div className="flex items-center justify-between gap-4" data-testid="welcome-hw-card">
          <div className="flex items-center gap-2.5 text-sm text-muted">
            <Spinner size={14} className="text-amber" />
            <span>{t.welcome.hwChecking}</span>
          </div>
        </div>
      </Card>
    )
  }

  if (!report) return null

  if (report.level === 'good') {
    const line = t.welcome.hwGoodLine(
      itemVal(report, 'cpu'),
      itemVal(report, 'memory'),
      itemVal(report, 'avail_memory'),
      itemVal(report, 'disk'),
    )
    return (
      <Card>
        <div
          className="flex items-center justify-between gap-4"
          data-testid="welcome-hw-card"
          data-hw-level="good"
        >
          <div className="flex items-center gap-2.5 text-sm text-body">
            <CheckCircle size={15} className="shrink-0 text-success" />
            <span className="tnum">{line}</span>
          </div>
          <Button size="sm" disabled={loading} onClick={onRecheck} data-testid="welcome-hw-recheck">
            {t.common.recheck}
          </Button>
        </div>
      </Card>
    )
  }

  const isInsufficient = report.level === 'insufficient'
  const badgeText = isInsufficient ? t.welcome.hwInsufficientBadge : t.welcome.hwTightBadge
  const borderCls = isInsufficient ? 'border-danger/40 bg-danger/5' : 'border-amber/40 bg-amber/5'
  const iconCls = isInsufficient ? 'text-danger' : 'text-amber-text'

  return (
    <Card className={borderCls}>
      <div
        className="flex items-start justify-between gap-4"
        data-testid="welcome-hw-card"
        data-hw-level={report.level}
      >
        <div className="flex items-start gap-2.5">
          <AlertTriangle size={16} className={`mt-[2px] shrink-0 ${iconCls}`} />
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-md font-medium text-ink">{report.headline}</span>
              <span className={`rounded px-2 py-0.5 text-xs font-medium ${iconCls} bg-surface`}>
                {badgeText}
              </span>
            </div>
            <div className="mt-1.5 text-sm leading-[1.55] text-body">{report.summary}</div>
          </div>
        </div>
        <Button size="sm" disabled={loading} onClick={onRecheck} data-testid="welcome-hw-recheck">
          {t.common.recheck}
        </Button>
      </div>

      <div className="mt-3 grid grid-cols-2 gap-x-6 gap-y-2 border-t border-line pt-3">
        {report.items.map((it) => {
          const dotCls =
            it.level === 'insufficient'
              ? 'bg-danger'
              : it.level === 'tight'
                ? 'bg-amber'
                : 'bg-success'
          return (
            <div key={it.id} className="flex items-start gap-2 text-xs leading-[1.45]">
              <span className={`mt-[5px] size-[6px] shrink-0 rounded-full ${dotCls}`} />
              <div className="min-w-0">
                <span className="font-medium text-ink">{it.label}：</span>
                <span className="tnum text-body">{it.value ?? t.app.noData}</span>
                <span className="ml-1.5 text-muted">（{it.note}）</span>
              </div>
            </div>
          )
        })}
      </div>
    </Card>
  )
}
