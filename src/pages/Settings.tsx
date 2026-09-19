import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { EnvList } from '../components/EnvList'
import { SegmentedControl, Toggle } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'
import { LOCALES, type Locale } from '../i18n'

/** 出站地址白名单（技术方案第 15 节；telemetry.agentpit.io 与 dl.agentpit.io 实测不存在，不列）。 */
const OUTBOUND = ['hunter.agentpit.io', 'ghcr.io', 'api.github.com', 'raw.githubusercontent.com']

export function Settings() {
  const { t, locale, setLocale, setOverlay } = useStore()
  const s = useAsync(() => ipc.readSettings(), [])
  const app = useAsync(() => ipc.appInfo(), [])
  const d = s.data
  const reason = s.error ? `${s.error.code} · ${s.error.message}` : t.app.noDataReason

  return (
    <PlainLayout
      title={t.settings.title}
      right={<Button size="sm" onClick={() => setOverlay(null)}>{t.common.close}</Button>}
    >
      <div className="grid grid-cols-2 gap-gap">
        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionGeneral}</div>
          <div className="mt-[16px] flex flex-col gap-[6px]">
            <Row label={t.settings.language}>
              <SegmentedControl<Locale>
                value={locale}
                onChange={setLocale}
                options={LOCALES.map((l) => ({ id: l.id, label: l.label }))}
              />
            </Row>
            <Row label={t.settings.autostart} hint={t.settings.autostartHint}>
              <Toggle on={d?.autostart ?? false} onChange={() => {}} label={t.settings.autostart} />
            </Row>
            <Row label={t.settings.checkUpdate}>
              <Toggle on={d?.checkUpdate ?? false} onChange={() => {}} label={t.settings.checkUpdate} />
            </Row>
          </div>
        </Card>

        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionDeploy}</div>
          <div className="mt-[16px]">
            <EnvList
              items={[
                { label: t.settings.registry, value: d?.registry ?? null, reason },
                { label: t.settings.hunterTag, value: d?.hunterTag ?? null, reason },
                { label: t.settings.workDir, value: d?.workDir ?? null, reason },
              ]}
            />
          </div>
          <div className="mt-[10px] text-xs leading-[1.5] text-muted">{t.settings.registryHint}</div>
          <div className="mt-[16px] flex gap-[10px]">
            <Button size="sm">{t.settings.resetKey}</Button>
          </div>
        </Card>

        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionPrivacy}</div>
          <div className="mt-[16px] flex flex-col gap-[6px]">
            <Row label={t.settings.telemetry} hint={t.settings.telemetryHint}>
              <Toggle on={d?.telemetry ?? false} onChange={() => {}} label={t.settings.telemetry} />
            </Row>
          </div>
          <div className="mt-[14px] text-sm text-label">{t.settings.outboundHint}</div>
          <ul className="tnum mt-[8px] flex flex-col gap-[6px] text-sm text-dim">
            {OUTBOUND.map((h) => (
              <li key={h}>{h}</li>
            ))}
          </ul>
          <div className="mt-[16px] flex gap-[10px]">
            <Button size="sm">{t.settings.viewQueue}</Button>
            <Button size="sm" onClick={() => setOverlay('feedback')}>
              {t.settings.exportDiag}
            </Button>
          </div>
        </Card>

        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionAbout}</div>
          <div className="mt-[16px]">
            <EnvList
              items={[
                { label: t.settings.version, value: app.data?.launcherVersion ?? null, reason },
                {
                  label: 'Docker',
                  value: app.data ? `${app.data.platform} · ${app.data.arch}` : null,
                  reason,
                },
                { label: t.settings.licenses, value: 'Noto Sans SC · JetBrains Mono · SIL OFL 1.1' },
              ]}
            />
          </div>
        </Card>
      </div>
    </PlainLayout>
  )
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-6 py-[7px]">
      <div className="min-w-0">
        <div className="text-md text-ink-2">{label}</div>
        {hint && <div className="mt-[5px] text-xs leading-[1.5] text-muted">{hint}</div>}
      </div>
      <div className="shrink-0 pt-[2px]">{children}</div>
    </div>
  )
}
