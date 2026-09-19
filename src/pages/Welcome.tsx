import { Button, ChevronRight } from '../components/Button'
import { Card } from '../components/Card'
import { Checkbox, Field, SegmentedControl } from '../components/Field'
import { WizardLayout } from '../components/WizardLayout'
import { useStore } from '../state/context'
import { LOCALES, type Locale } from '../i18n'
import { useState } from 'react'

export function Welcome() {
  const { t, locale, setLocale, send } = useStore()
  const [agreed, setAgreed] = useState(false)

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
      <div className="mt-[34px] flex flex-col gap-[26px]">
        <Field label={t.welcome.languageLabel}>
          <SegmentedControl<Locale>
            value={locale}
            onChange={setLocale}
            options={LOCALES.map((l) => ({ id: l.id, label: l.label }))}
          />
        </Field>

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
