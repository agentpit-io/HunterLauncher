import { useState } from 'react'
import { Button } from '../components/Button'
import { Card } from '../components/Card'
import { EnvList } from '../components/EnvList'
import { Checkbox, Field, SegmentedControl, TextInput, Toggle } from '../components/Field'
import { Modal } from '../components/Modal'
import { OfflineImport } from '../components/OfflineImport'
import { PlainLayout } from '../components/WizardLayout'
import { UploadLogs } from '../components/UploadLogs'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'
import { LOCALES, type Locale } from '../i18n'
import type { BackupSettings, LauncherSettings } from '../lib/types'

/**
 * 出站地址白名单（技术方案第 15 节）。
 * 方案里写的 telemetry.agentpit.io 与 dl.agentpit.io **实测不存在**（M0），列上去是误导，去掉。
 * 这里列的是启动器**真的会连**的：网关、两个候选镜像源、Docker Hub（postgres/redis 的 manifest）、
 * GitHub（版本检查与 compose 文件）。自带模型 key 模式下还会连用户自己填的那个 BASE_URL。
 */
const OUTBOUND = [
  'hunter.agentpit.io',
  // I16：一键上传日志的接收地址。**只有用户点了那个按钮、或者开了「出错时自动上传」
  // 并且真的出了错**，启动器才会连这里 —— 但它确实会连，所以这份清单里必须有它
  'www.agentpit.io',
  'ghcr.io',
  'hkccr.ccs.tencentyun.com',
  'registry-1.docker.io',
  'api.github.com',
  'raw.githubusercontent.com',
]

export function Settings() {
  const { t, locale, setLocale, setOverlay } = useStore()
  const s = useAsync(() => ipc.readSettings(), [])
  const app = useAsync(() => ipc.appInfo(), [])
  const [probeNonce, setProbeNonce] = useState(0)
  const probes = useAsync(() => ipc.probeRegistries(), [probeNonce])
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState<LauncherSettings | null>(null)
  const [showQueue, setShowQueue] = useState(false)
  const d = saved ?? s.data
  const reason = s.error ? `${s.error.code} · ${s.error.message}` : t.app.noDataReason

  /** 改一项就落一次盘。写回 launcher.toml 是真的（M2 起不再是空函数）。 */
  async function patch(p: Partial<LauncherSettings>) {
    if (!d) return
    setSaving(true)
    try {
      setSaved(await ipc.writeSettings({ ...d, ...p }))
    } catch {
      /* 失败就保持原值，下面的 reason 会说明 */
    } finally {
      setSaving(false)
    }
  }

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
                testIdPrefix="settings-locale"
                onChange={(l) => {
                  setLocale(l)
                  void patch({ locale: l })
                }}
                options={LOCALES.map((l) => ({ id: l.id, label: l.label }))}
              />
            </Row>
            {/* 开机自启：Toggle 的值来自**系统里真实的自启项**（Rust 侧 autostart::status），
                不是配置文件里记的那个布尔（红线 1）。 */}
            <Row
              label={t.settings.autostart}
              hint={`${t.settings.autostartHint} ${d ? t.settings.autostartReal(d.autostart) : ''}`}
            >
              <Toggle
                on={d?.autostart ?? false}
                testId="settings-autostart"
                onChange={(v) => void patch({ autostart: v })}
                label={t.settings.autostart}
              />
            </Row>
            <Row label={t.settings.checkUpdate}>
              <Toggle
                on={d?.checkUpdate ?? false}
                testId="settings-checkupdate"
                onChange={(v) => void patch({ checkUpdate: v })}
                label={t.settings.checkUpdate}
              />
            </Row>
          </div>
        </Card>

        <RegistryCard
          settings={d}
          reason={reason}
          probes={probes}
          saving={saving}
          onRetest={() => setProbeNonce((n) => n + 1)}
          onPatch={patch}
        />

        <ModelCard settings={d} onDone={() => s.reload()} />

        {/* AI 助手（I4 §三 + I5 §3.2 的三档授权）。
            **档位是这一节的主开关**：I4 那个布尔开关现在由档位推出（off ⇔ 关），
            两个来源写同一件事只会互相打架，所以界面上只留档位这一处。 */}
        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionAssist}</div>
          <div className="mt-[6px] text-sm leading-[1.6] text-muted">{t.settings.assistModeHint}</div>
          <div className="mt-[14px] flex flex-col gap-[8px]">
            {(
              [
                ['auto', t.settings.assistModeAuto, t.settings.assistModeAutoHint],
                ['confirm', t.settings.assistModeConfirm, t.settings.assistModeConfirmHint],
                ['off', t.settings.assistModeOff, t.settings.assistModeOffHint],
              ] as const
            ).map(([value, label, hint]) => {
              const on = (d?.assistMode ?? 'auto') === value
              return (
                <button
                  key={value}
                  type="button"
                  data-testid={`settings-assist-mode-${value}`}
                  onClick={() => void patch({ assistMode: value, assist: value !== 'off' })}
                  className={`flex items-start gap-[10px] rounded-md border px-[14px] py-[11px] text-left transition-colors ${
                    on ? 'border-amber bg-amber-soft' : 'border-line bg-card hover:bg-hover'
                  }`}
                >
                  <span
                    className={`mt-[4px] size-[12px] shrink-0 rounded-full border ${
                      on ? 'border-amber bg-amber' : 'border-line-strong'
                    }`}
                    aria-hidden
                  />
                  <span className="min-w-0">
                    <span className="block text-md leading-tight text-ink">{label}</span>
                    <span className="mt-[4px] block text-xs leading-[1.5] text-muted">{hint}</span>
                  </span>
                </button>
              )
            })}
          </div>
          <div className="mt-[10px] text-xs leading-[1.6] text-muted">
            {d?.assistConsentedAt
              ? t.settings.assistConsentedAt(d.assistConsentedAt)
              : t.settings.assistConsentedNever}
          </div>
          <div className="mt-[10px] text-xs leading-[1.6] text-muted">{t.settings.assistNote}</div>
          <AuditLog t={t} />
          <div className="mt-[14px]">
            <EnvList
              items={[
                // 「启动器现在用的是哪一个 docker」—— I4 那个 macOS P0 之后必须能一眼看到
                { label: t.settings.dockerPath, value: d?.dockerPath ?? null, reason: t.settings.dockerPathNone },
              ]}
            />
          </div>
        </Card>

        {/* I7 · 容器运行时：没有 Docker 时走哪条路，以及内置运行时的现状与卸载。
            这一节里的每个数字（装了什么版本、多少字节、从哪个源下的）都来自
            Rust 当场读的 `installed.json`，界面不生成任何数字（红线 1）。 */}
        <RuntimeCard settings={d} t={t} onPatch={patch} onDone={() => s.reload()} />

        {/* I13 · R6：数据备份。开关、时间、目录、保留天数、内容勾选都在这里，
            改完**当场**去改系统里的定时任务 —— 「配置里写着几点」和
            「系统里真的几点跑」永远不该是两件事（写在 write_backup_settings 里）。 */}
        <BackupCard t={t} onOpen={() => setOverlay('backup')} />

        {/* I13 · R7：资源提醒。阈值都是 `[monitor]` 里的真实配置，
            右边那一行「现在有几条提醒」是当场跑一遍监测的结果，不是缓存 */}
        <MonitorCard t={t} />

        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionPrivacy}</div>
          <div className="mt-[16px] flex flex-col gap-[6px]">
            <Row label={t.settings.telemetry} hint={t.settings.telemetryHint}>
              <Toggle
                on={d?.telemetry ?? false}
                testId="settings-telemetry"
                onChange={(v) => void patch({ telemetry: v })}
                label={t.settings.telemetry}
              />
            </Row>
          </div>
          {/* 上报端点：默认空，界面上**如实写「暂未开启上报」**（里程碑 M3 第 6 项） */}
          <div className="mt-[10px] flex items-baseline justify-between gap-4">
            <span className="shrink-0 text-md text-label">{t.settings.telemetryEndpoint}</span>
            <span className="min-w-0 truncate text-md text-amber-text" data-testid="telemetry-endpoint">
              {d?.telemetryEndpoint ? d.telemetryEndpoint : t.settings.telemetryNoEndpoint}
            </span>
          </div>
          <div className="mt-[6px] text-xs leading-[1.6] text-muted">
            {t.settings.telemetryNoEndpointHint}
          </div>

          <div className="mt-[14px] text-sm text-label">{t.settings.outboundHint}</div>
          <ul className="tnum mt-[8px] flex flex-col gap-[6px] text-sm text-dim">
            {OUTBOUND.map((h) => (
              <li key={h}>{h}</li>
            ))}
          </ul>
          {/* I16 · 一键上传日志。开关默认关，而且打开之后**只在出错时**传 ——
              这一条和上面那个遥测开关是两回事：遥测是「平时的事件」，
              这一条是「出事了那一份现场」，而且永远由用户点或者由错误触发。 */}
          <div className="mt-[18px] border-t border-line pt-[16px]">
            <Row label={t.logship.autoLabel} hint={t.logship.autoHint}>
              <Toggle
                on={d?.autoUploadLogs ?? false}
                testId="settings-auto-upload"
                onChange={(v) => void patch({ autoUploadLogs: v })}
                label={t.logship.autoLabel}
              />
            </Row>
            <div className="tnum mt-[10px] text-xs text-muted" data-testid="last-trace">
              {d?.lastTraceCode
                ? t.logship.lastTrace(d.lastTraceCode, d.lastUploadAt)
                : t.logship.lastTraceNever}
            </div>
          </div>

          <div className="mt-[16px] flex flex-wrap gap-[10px]">
            <Button size="sm" data-testid="view-queue" onClick={() => setShowQueue(true)}>
              {t.settings.viewQueue}
            </Button>
            <Button size="sm" onClick={() => setOverlay('feedback')}>
              {t.settings.exportDiag}
            </Button>
            <UploadLogs stage="settings" testId="settings-upload-logs" />
          </div>
        </Card>

        <Card>
          <div className="text-md font-medium text-ink">{t.settings.sectionAbout}</div>
          <div className="mt-[16px]">
            <EnvList
              items={[
                { label: t.settings.version, value: app.data?.launcherVersion ?? null, reason },
                {
                  label: t.settings.system,
                  value: app.data ? `${app.data.platform} · ${app.data.arch}` : null,
                  reason,
                },
                { label: t.settings.hunterTag, value: d?.hunterTag ?? null, reason },
                { label: t.settings.workDir, value: d?.workDir ?? null, reason },
                { label: t.settings.licenses, value: 'Noto Sans SC · JetBrains Mono · SIL OFL 1.1' },
              ]}
            />
          </div>
          {/* 更新与备份都在更新页里（方案 §10）。这里只留一个入口，不把两套界面抄两遍 */}
          <div className="mt-[16px] flex flex-wrap gap-[10px]">
            <Button size="sm" data-testid="settings-update" onClick={() => setOverlay('update')}>
              {t.update.hunterCheck}
            </Button>
            <OfflineImport variant="button" />
          </div>
        </Card>
      </div>

      {showQueue && <TelemetryQueue onClose={() => setShowQueue(false)} />}
    </PlainLayout>
  )
}

/**
 * 容器运行时（I7）。
 *
 * 两件事：①「电脑上没有 Docker 时走哪条路」——默认内置运行时（零点击），
 * 备选 OrbStack（首次启动会弹系统提示，做不到零点击，这里如实写明）；
 * ② 内置运行时装了没有、装了什么、以及**一键卸载**。
 */
function RuntimeCard({
  settings,
  t,
  onPatch,
  onDone,
}: {
  settings: LauncherSettings | null
  t: ReturnType<typeof useStore>['t']
  onPatch: (p: Partial<LauncherSettings>) => Promise<void>
  onDone: () => void
}) {
  const [nonce, setNonce] = useState(0)
  const st = useAsync(() => ipc.builtinRuntimeStatus(), [nonce])
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)

  async function uninstall() {
    setBusy(true)
    setErr(null)
    setMsg(null)
    try {
      setMsg(await ipc.builtinRuntimeUninstall())
      setNonce((n) => n + 1)
      onDone()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
      setConfirming(false)
    }
  }

  const d = st.data
  return (
    <Card>
      <div className="text-md font-medium text-ink">{t.settings.sectionRuntime}</div>
      <div className="mt-[6px] text-sm leading-[1.6] text-muted">{t.settings.runtimeHint}</div>

      <div className="mt-[14px] flex flex-col gap-[8px]">
        {(
          [
            ['builtin', t.settings.routeBuiltin, t.settings.routeBuiltinHint],
            ['orbstack', t.settings.routeOrbstack, t.settings.routeOrbstackHint],
          ] as const
        ).map(([value, label, hint]) => {
          const on = (settings?.installRoute ?? 'builtin') === value
          return (
            <button
              key={value}
              type="button"
              data-testid={`settings-install-route-${value}`}
              onClick={() => void onPatch({ installRoute: value })}
              className={`flex items-start gap-[10px] rounded-md border px-[14px] py-[11px] text-left transition-colors ${
                on ? 'border-amber bg-amber-soft' : 'border-line bg-card hover:bg-hover'
              }`}
            >
              <span
                className={`mt-[4px] size-[12px] shrink-0 rounded-full border ${
                  on ? 'border-amber bg-amber' : 'border-line-strong'
                }`}
                aria-hidden
              />
              <span className="min-w-0">
                <span className="block text-md leading-tight text-ink">{label}</span>
                <span className="mt-[4px] block text-xs leading-[1.5] text-muted">{hint}</span>
              </span>
            </button>
          )
        })}
      </div>

      <label className="mt-[12px] flex cursor-pointer items-start gap-[10px]">
        <input
          type="checkbox"
          data-testid="settings-allow-install-runtime"
          className="mt-[3px] size-[15px] shrink-0 accent-amber"
          checked={settings?.allowInstallRuntime ?? true}
          onChange={(e) => void onPatch({ allowInstallRuntime: e.target.checked })}
        />
        <span className="min-w-0">
          <span className="block text-md leading-tight text-ink">{t.consent.allowInstallTitle}</span>
          <span className="mt-[4px] block text-xs leading-[1.5] text-muted">{t.settings.allowInstallHint}</span>
        </span>
      </label>

      {/* 内置运行时的现状。**不支持时把原因原样显示出来**，不含糊成一句「不可用」 */}
      <div className="mt-[16px]" data-testid="settings-builtin-runtime">
        {st.loading && <div className="text-sm text-muted">{t.common.loading}</div>}
        {st.error && <div className="text-sm text-danger">{st.error.message}</div>}
        {d && !d.supported && (
          <div className="rounded-md border border-line bg-card px-[14px] py-[11px] text-sm leading-[1.6] text-muted">
            {d.unsupportedReason}
          </div>
        )}
        {d && d.supported && !d.installed && (
          <div className="rounded-md border border-line bg-card px-[14px] py-[11px] text-sm leading-[1.6] text-muted">
            {t.settings.builtinNotInstalled(d.dir, fmtBytes(d.downloadBytes))}
          </div>
        )}
        {d && d.installed && (
          <div className="rounded-md border border-line bg-card px-[14px] py-[11px]">
            <div className="flex flex-wrap items-baseline gap-x-[14px] gap-y-[4px]">
              <span className="text-md text-ink">{t.settings.builtinInstalled}</span>
              <span className="text-sm text-body">
                {d.running ? t.settings.builtinRunning : t.settings.builtinStopped}
              </span>
              {d.installedAt && <span className="font-mono text-xs text-muted">{d.installedAt}</span>}
            </div>
            <ul className="mt-[8px] flex flex-col gap-[3px]">
              {d.items.map((it) => (
                <li key={it.file} className="font-mono text-xs leading-[1.6] text-dim">
                  {it.component} {it.version} · {fmtBytes(it.bytes)} · {it.source} · sha256 {it.sha256.slice(0, 12)}…
                </li>
              ))}
            </ul>
            <div className="mt-[8px] font-mono text-xs text-muted">{d.dir}</div>
            <div className="mt-[12px] flex flex-wrap items-center gap-[10px]">
              {!confirming ? (
                <Button
                  size="sm"
                  data-testid="settings-uninstall-runtime"
                  disabled={busy}
                  onClick={() => setConfirming(true)}
                >
                  {t.settings.uninstallRuntime}
                </Button>
              ) : (
                <>
                  <span className="text-sm leading-[1.5] text-body">{t.settings.uninstallConfirm}</span>
                  <Button
                    size="sm"
                    variant="primary"
                    data-testid="settings-uninstall-runtime-yes"
                    disabled={busy}
                    onClick={() => void uninstall()}
                  >
                    {busy ? t.common.working : t.settings.uninstallYes}
                  </Button>
                  <Button size="sm" disabled={busy} onClick={() => setConfirming(false)}>
                    {t.common.cancel}
                  </Button>
                </>
              )}
            </div>
          </div>
        )}
        {msg && (
          <div className="mt-[10px] break-all text-sm leading-[1.5] text-amber-text" data-testid="settings-runtime-msg">
            {msg}
          </div>
        )}
        {err && <div className="mt-[10px] break-all text-sm text-danger">{err}</div>}
      </div>
    </Card>
  )
}

/** 字节数 → 人话。**只在这里做格式化，数字本身来自 Rust。** */
function fmtBytes(n: number): string {
  if (n <= 0) return '—'
  const mb = n / (1024 * 1024)
  return mb >= 1024 ? `${(mb / 1024).toFixed(2)} GiB` : `${mb.toFixed(1)} MiB`
}

/**
 * 镜像源：显示两个候选源的**真实探测结果**，并且允许手动固定（里程碑 M3 第 3 项）。
 * 自定义前缀这一档是给自建仓库的内网用户的 —— 填了就不测速（不知道仓库路径，探不了）。
 */
function RegistryCard({
  settings,
  reason,
  probes,
  saving,
  onRetest,
  onPatch,
}: {
  settings: LauncherSettings | null
  reason: string
  probes: ReturnType<typeof useAsync<Awaited<ReturnType<typeof ipc.probeRegistries>>>>
  saving: boolean
  onRetest: () => void
  onPatch: (p: Partial<LauncherSettings>) => Promise<void>
}) {
  const { t } = useStore()
  const [custom, setCustom] = useState('')
  // I7：只有「升级前就对局域网开放」的老机器会用到这两个 state
  const [tightening, setTightening] = useState(false)
  const [tightenMsg, setTightenMsg] = useState('')

  async function onTighten() {
    setTightening(true)
    setTightenMsg('')
    try {
      setTightenMsg(await ipc.tightenWebBind())
      // 收紧之后 webLanExposed 变成 false，那段说明与按钮会一起消失
      await onPatch({})
    } catch (e) {
      setTightenMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setTightening(false)
    }
  }

  return (
    <Card>
      <div className="flex items-center justify-between">
        <div className="text-md font-medium text-ink">{t.settings.sectionDeploy}</div>
        <Button size="sm" data-testid="registry-retest" onClick={onRetest} disabled={probes.loading}>
          {probes.loading ? t.settings.registryTesting : t.settings.registryRetest}
        </Button>
      </div>
      <div className="mt-[16px]">
        <EnvList
          items={[
            { label: t.settings.registry, value: settings?.registry ?? null, reason },
            { label: t.settings.registryPin, value: settings?.registryPrefix ?? null, reason },
          ]}
        />
      </div>
      <div className="mt-[12px]">
        <SegmentedControl<string>
          value={settings?.registry ?? 'ghcr'}
          testIdPrefix="registry"
          onChange={(v) => void onPatch({ registry: v })}
          options={(probes.data ?? []).map((r) => ({
            id: r.id,
            label: `${r.label} · ${r.available ? `${r.elapsedMs} ms` : t.settings.registryDown}`,
          }))}
        />
      </div>
      <div className="mt-[10px] text-xs leading-[1.5] text-muted">
        {probes.error ? `${probes.error.code} · ${probes.error.message}` : t.settings.registryHint}
      </div>

      <Field className="mt-[14px]" label={t.settings.registryCustom} hint={t.settings.registryCustomHint}>
        <div className="flex gap-[10px]">
          <TextInput
            value={custom}
            onChange={setCustom}
            mono
            placeholder="registry.example.com/agentpit"
          />
          <Button
            size="sm"
            data-testid="registry-custom-apply"
            disabled={!custom.includes('/')}
            onClick={() => void onPatch({ registry: custom.trim() })}
          >
            {t.common.apply}
          </Button>
        </div>
      </Field>

      {/* 谁能打开 Hunter（待办池 P1-20 · 用户 2026-09-21 19:05 拍板后**不再是开关**）。
          原话：「目前只能本机访问，不考虑同一局域网访问，这个需要升级付费版本才可以。」
          所以这里是一行只读的说明 + 一句付费版提示，**点不动、也不写配置**。
          唯一的例外是升级上来的老机器（升级前就对局域网开放，本轮有意没动它）：
          多出一段说明与一个「只允许本机访问」按钮，收紧是单向的。 */}
      <div className="mt-[14px] border-t border-line pt-[14px]">
        <div className="text-md text-ink-2">{t.settings.webAccess}</div>
        <div
          className="mt-[10px] inline-flex items-center rounded-[6px] border border-line px-[12px] py-[6px] text-md text-ink"
          data-testid="web-access-value"
        >
          {t.settings.webAccessValue}
        </div>
        <div className="mt-[10px] text-xs leading-[1.5] text-muted">{t.settings.webAccessHint}</div>
        <div className="mt-[6px] text-xs leading-[1.5] text-muted" data-testid="web-access-paid">
          {t.settings.webAccessPaid}
        </div>
        {settings?.webLanExposed && (
          <div className="mt-[10px] rounded-[6px] border border-amber/40 bg-amber/5 p-[12px]">
            <div className="text-xs leading-[1.5] text-amber-text">{t.settings.webAccessLegacy}</div>
            <div className="mt-[10px] flex items-center gap-[10px]">
              <Button
                size="sm"
                data-testid="web-access-tighten"
                disabled={tightening}
                onClick={() => void onTighten()}
              >
                {tightening ? t.settings.webAccessTightening : t.settings.webAccessTighten}
              </Button>
              <span className="text-xs leading-[1.5] text-muted">
                {t.settings.webAccessTightenNote}
              </span>
            </div>
            {tightenMsg && (
              <div className="mt-[8px] text-xs leading-[1.5] text-ink-2" data-testid="web-access-tighten-msg">
                {tightenMsg}
              </div>
            )}
          </div>
        )}
      </div>

      {saving && <div className="mt-[8px] text-xs text-amber-text">{t.settings.saving}</div>}
    </Card>
  )
}

/**
 * 模型模式切换（里程碑 M3 第 3 项）。
 *
 * 点「切换」会走 Rust 的 `switch_model`：先真发一次请求验连通性，通过之后重写 `.env`
 * 并 `up -d --force-recreate api opencode llm-shim`，再等这三个服务健康。
 * **要几十秒**，所以按钮按下去之后一直显示「正在重写 .env 并重建容器…」直到 Rust 回话。
 */
function ModelCard({ settings, onDone }: { settings: LauncherSettings | null; onDone: () => void }) {
  const { t } = useStore()
  const [mode, setMode] = useState<'gateway' | 'own'>('gateway')
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)

  // 首次拿到设置时把当前模式同步过来（之后由用户自己切）
  const current = settings?.modelMode ?? 'gateway'
  const [synced, setSynced] = useState(false)
  if (settings && !synced) {
    setSynced(true)
    setMode(current === 'own' ? 'own' : 'gateway')
    setBaseUrl(settings.modelBaseUrl)
    setModel(settings.modelName)
  }

  async function doSwitch() {
    setBusy(true)
    setMsg(null)
    try {
      setMsg(
        await ipc.switchModel(
          mode === 'own' ? { mode: 'own', baseUrl, model, apiKey } : { mode: 'gateway' },
        ),
      )
      // key 用完就从界面上抹掉，不在 React state 里长期留着（红线 2）
      setApiKey('')
      onDone()
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card>
      <div className="text-md font-medium text-ink">{t.settings.sectionModel}</div>
      <div className="mt-[6px] text-xs text-muted">
        {t.settings.modelCurrent(
          current === 'own' ? t.settings.modelOwn : t.settings.modelGateway,
          settings?.modelName || '—',
        )}
      </div>
      <div className="mt-[14px]">
        <SegmentedControl<'gateway' | 'own'>
          value={mode}
          testIdPrefix="model-mode"
          onChange={setMode}
          options={[
            { id: 'gateway', label: t.settings.modelGateway },
            { id: 'own', label: t.settings.modelOwn },
          ]}
        />
      </div>
      <div className="mt-[10px] text-xs leading-[1.6] text-muted">
        {mode === 'own' ? t.settings.modelOwnHint : t.settings.modelGatewayHint}
      </div>

      {mode === 'own' && (
        <div className="mt-[14px] flex flex-col gap-[12px]">
          <Field label={t.settings.modelBaseUrl}>
            <TextInput value={baseUrl} onChange={setBaseUrl} mono placeholder="https://api.deepseek.com/v1" />
          </Field>
          <Field label={t.settings.modelName}>
            <TextInput value={model} onChange={setModel} mono placeholder="deepseek-chat" />
          </Field>
          <Field label={t.settings.modelApiKey}>
            <TextInput value={apiKey} onChange={setApiKey} password placeholder="sk-…" />
          </Field>
        </div>
      )}

      <div className="mt-[16px]">
        <Button
          size="sm"
          variant="primary"
          data-testid="model-switch"
          disabled={busy || (mode === 'own' && (!baseUrl || !model || !apiKey))}
          onClick={() => void doSwitch()}
        >
          {busy ? t.settings.modelSwitching : t.settings.modelSwitch}
        </Button>
      </div>
      {msg && (
        <div className="mt-[12px] break-all text-sm leading-[1.5] text-amber-text" data-testid="model-switch-msg">
          {msg}
        </div>
      )}
    </Card>
  )
}

/**
 * 「查看本机将要发送的数据」（方案 §12.1 的「透明」那一条）。
 * 显示的是 `queue.jsonl` 的**原文**，一个字都不加工。
 */
function TelemetryQueue({ onClose }: { onClose: () => void }) {
  const { t } = useStore()
  const q = useAsync(() => ipc.telemetryView(), [])
  const [cleared, setCleared] = useState(false)
  const lines = cleared ? [] : (q.data?.lines ?? [])

  return (
    <Modal
      testId="telemetry-queue"
      title={t.settings.viewQueue}
      onClose={onClose}
      footer={
        <>
          <Button
            size="sm"
            data-testid="telemetry-clear"
            onClick={() => {
              void ipc.telemetryClear().then(() => setCleared(true))
            }}
          >
            {t.settings.telemetryClear}
          </Button>
          <Button size="sm" variant="primary" onClick={onClose}>
            {t.common.close}
          </Button>
        </>
      }
    >
      <div className="text-sm text-muted">
        {t.settings.telemetryEndpoint}：
        <span className="text-amber-text">
          {q.data?.endpoint ? q.data.endpoint : t.settings.telemetryNoEndpoint}
        </span>
      </div>
      <div className="mt-[6px] text-xs text-muted">{t.settings.telemetryOffClears}</div>
      {q.data && (
        <div className="tnum mt-[10px] break-all text-xs text-muted">
          {t.settings.telemetryQueuePath(q.data.queuePath)}
        </div>
      )}
      <div className="mt-[14px] text-sm text-label">{t.settings.telemetryEvents}</div>
      <div className="mt-[6px] flex flex-wrap gap-[6px]">
        {(q.data?.events ?? []).map((e) => (
          <span key={e} className="tnum rounded border border-line-strong px-2 py-[2px] text-xs text-dim">
            {e}
          </span>
        ))}
      </div>
      <pre className="tnum selectable mt-[14px] max-h-[240px] overflow-auto whitespace-pre-wrap break-all rounded-md border border-line bg-log px-3 py-2.5 text-xs leading-[1.5] text-dim">
        {lines.length > 0 ? lines.join('\n') : t.settings.telemetryQueueEmpty}
      </pre>
    </Modal>
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


/**
 * 「AI 都做过什么」（I5 §3.3）。
 *
 * 承诺「绝不会做 X」只有在用户**查得到它到底做了什么**的时候才站得住，
 * 所以审计日志要能在界面上直接看到，而不是只躺在一个要自己 cat 的文件里。
 * 内容由 Rust 侧写入时就脱敏过（key 不会出现在里面）。
 */
function AuditLog({ t }: { t: ReturnType<typeof useStore>['t'] }) {
  const [lines, setLines] = useState<string[] | null>(null)
  const [busy, setBusy] = useState(false)
  return (
    <div className="mt-[14px] border-t border-line pt-[14px]">
      <div className="flex items-center justify-between">
        <div>
          <div className="text-sm text-body">{t.settings.auditTitle}</div>
          <div className="mt-[3px] text-xs leading-[1.5] text-muted">{t.settings.auditHint}</div>
        </div>
        <Button
          size="sm"
          variant="ghost"
          data-testid="settings-audit-toggle"
          disabled={busy}
          onClick={() => {
            if (lines) {
              setLines(null)
              return
            }
            setBusy(true)
            void ipc
              .assistAuditTail(20)
              .then(setLines)
              .catch(() => setLines([]))
              .finally(() => setBusy(false))
          }}
        >
          {lines ? t.settings.auditHide : t.settings.auditShow}
        </Button>
      </div>
      {lines && (
        <pre
          data-testid="settings-audit"
          className="mt-[10px] max-h-[220px] overflow-auto whitespace-pre-wrap break-all rounded-md bg-log px-[12px] py-[9px] font-mono text-xs leading-[1.55] text-dim"
        >
          {lines.length > 0 ? lines.join('\n') : t.settings.auditEmpty}
        </pre>
      )}
    </div>
  )
}

/**
 * 「数据备份」分区（I13 · R6 6.1）。
 *
 * 界面上的每一项都对应 `launcher.toml [backup]` 里的一项，改一项落一次盘，
 * **并且当场把系统里的定时任务改成一致的样子**。
 *
 * 「定时任务」那一行显示的是 Rust 现查系统的结果（`launchctl print` /
 * `systemctl --user is-enabled` / `schtasks /Query`），不是配置里的备忘 ——
 * 配置说「开着」而系统里根本没装，正是最需要被看见的那种情况。
 */
function BackupCard({ t, onOpen }: { t: ReturnType<typeof useStore>['t']; onOpen: () => void }) {
  const [nonce, setNonce] = useState(0)
  const b = useAsync(() => ipc.readBackupSettings(), [nonce])
  const sc = useAsync(() => ipc.backupScheduleStatus(), [nonce])
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [draft, setDraft] = useState<BackupSettings | null>(null)
  const d = draft ?? b.data

  async function patch(p: Partial<BackupSettings>) {
    if (!d) return
    setSaving(true)
    setErr(null)
    const next = { ...d, ...p }
    setDraft(next)
    try {
      setDraft(await ipc.writeBackupSettings(next))
      setNonce((n) => n + 1)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
      setDraft(b.data)
    } finally {
      setSaving(false)
    }
  }

  async function pick() {
    try {
      const p = await ipc.pickBackupDir()
      if (p) await patch({ dir: p })
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <Card>
      <div className="text-md font-medium text-ink">{t.settings.sectionBackup}</div>
      <div className="mt-[6px] text-sm leading-[1.6] text-muted">{t.settings.backupHint}</div>
      <div className="mt-[14px] flex flex-col gap-[6px]">
        <Row label={t.settings.backupEnabled}>
          <Toggle
            on={d?.enabled ?? false}
            testId="settings-backup-enabled"
            onChange={(v) => void patch({ enabled: v })}
            label={t.settings.backupEnabled}
          />
        </Row>
      </div>
      <Field className="mt-[14px]" label={t.settings.backupTime} hint={t.settings.backupTimeHint}>
        <TextInput
          value={d?.time ?? ''}
          mono
          onChange={(v) => setDraft(d ? { ...d, time: v } : d)}
          placeholder="00:00"
        />
        <div className="mt-[8px]">
          <Button
            size="sm"
            data-testid="settings-backup-time-save"
            disabled={saving || !d}
            onClick={() => d && void patch({ time: d.time })}
          >
            {t.common.save}
          </Button>
        </div>
      </Field>
      <Field className="mt-[14px]" label={t.settings.backupDir} hint={t.settings.backupDirHint}>
        <div className="tnum break-all text-md text-ink" data-testid="settings-backup-dir">
          {d?.effectiveDir ?? t.app.noData}
        </div>
        <div className="mt-[8px] flex flex-wrap gap-[10px]">
          <Button size="sm" data-testid="settings-backup-pick" onClick={() => void pick()}>
            {t.settings.backupDirPick}
          </Button>
          {d?.dir && (
            <Button size="sm" variant="ghost" onClick={() => void patch({ dir: '' })}>
              {t.settings.backupDirSuggest(d.suggestedDir)}
            </Button>
          )}
        </div>
        {d?.externalSuggestions.map((x) => (
          <div key={x} className="mt-[8px] text-xs leading-[1.5] text-amber-text">
            {t.settings.backupDirExternal(x)}
          </div>
        ))}
      </Field>
      <Field className="mt-[14px]" label={t.settings.backupKeepDays} hint={t.settings.backupKeepDaysHint}>
        <SegmentedControl<string>
          value={String(d?.keepDays ?? 3)}
          testIdPrefix="settings-backup-keep"
          onChange={(v) => void patch({ keepDays: Number(v) })}
          options={['1', '3', '7', '14', '30'].map((n) => ({ id: n, label: n }))}
        />
      </Field>
      <div className="mt-[14px] text-sm text-label">{t.settings.backupIncludes}</div>
      <div className="mt-[8px] flex flex-col gap-[8px]">
        <Checkbox on disabled onChange={() => {}}>
          {t.settings.backupIncludeDb}
        </Checkbox>
        <Checkbox on disabled onChange={() => {}}>
          {t.settings.backupIncludeSecrets}
        </Checkbox>
        <Checkbox on disabled onChange={() => {}}>
          {t.settings.backupIncludeEnv}
        </Checkbox>
        <Checkbox
          on={d?.includeSkills ?? true}
          onChange={(v) => void patch({ includeSkills: v })}
        >
          <span data-testid="settings-backup-skills">{t.settings.backupIncludeSkills}</span>
        </Checkbox>
        <Checkbox
          on={d?.includeSessions ?? true}
          onChange={(v) => void patch({ includeSessions: v })}
        >
          <span data-testid="settings-backup-sessions">{t.settings.backupIncludeSessions}</span>
        </Checkbox>
      </div>
      <div className="mt-[14px]">
        <EnvList
          items={[
            {
              label: t.settings.backupSchedule,
              value: sc.data
                ? `${sc.data.mech} · ${sc.data.installed ? (sc.data.enabled ? 'enabled' : 'installed') : 'not installed'}`
                : null,
              reason: sc.data?.reason || t.app.noDataReason,
            },
            {
              label: t.settings.backupLastOk,
              value: d?.lastOkAt || null,
              reason: t.settings.backupNever,
            },
            {
              label: t.settings.backupLastError,
              value: d?.lastError || null,
              reason: '—',
            },
          ]}
        />
      </div>
      {sc.data?.nextRun && (
        <div className="tnum mt-[8px] break-all text-xs text-muted">{sc.data.nextRun}</div>
      )}
      {sc.data && !sc.data.supported && (
        <div className="mt-[8px] text-xs leading-[1.5] text-amber-text">{sc.data.reason}</div>
      )}
      {err && <div className="mt-[10px] text-sm leading-[1.5] text-danger">{err}</div>}
      {saving && <div className="mt-[8px] text-xs text-amber-text">{t.settings.backupSaving}</div>}
      <div className="mt-[14px]">
        <Button size="sm" data-testid="settings-open-backup" onClick={onOpen}>
          {t.settings.backupOpen}
        </Button>
      </div>
    </Card>
  )
}

/** 「资源提醒」分区（I13 · R7）。阈值是只读展示 —— 改它们要动 `[monitor]`，
 *  而那几个数字的含义比一个滑块能表达的多，写进配置文件更诚实。 */
function MonitorCard({ t }: { t: ReturnType<typeof useStore>['t'] }) {
  const a = useAsync(() => ipc.monitorAlerts(), [])
  const c = useAsync(() => ipc.cleanupPlan(), [])
  return (
    <Card>
      <div className="text-md font-medium text-ink">{t.settings.sectionMonitor}</div>
      <div className="mt-[6px] text-sm leading-[1.6] text-muted">{t.settings.monitorHint}</div>
      <div className="mt-[14px]">
        <EnvList
          items={[
            {
              label: t.settings.monitorAlerts,
              value: a.data ? (a.data.alerts.length === 0 ? t.settings.monitorNone : String(a.data.alerts.length)) : null,
              reason: a.error?.message ?? t.app.noDataReason,
            },
            {
              label: t.settings.monitorCleanup,
              value: c.data ? fmtBytes(c.data.totalBytes) : null,
              reason: c.data?.reasons[0] ?? t.app.noDataReason,
            },
          ]}
        />
      </div>
      {a.data && a.data.alerts.length > 0 && (
        <ul className="mt-[10px] flex flex-col gap-[6px] text-sm leading-[1.5] text-body" data-testid="settings-alerts">
          {a.data.alerts.map((x) => (
            <li key={x.id}>
              · <span className={x.level === 'crit' ? 'text-danger' : 'text-amber-text'}>{x.title}</span>
            </li>
          ))}
        </ul>
      )}
      {a.data && Object.entries(a.data.reasons).length > 0 && (
        <ul className="mt-[8px] flex flex-col gap-[4px] text-xs leading-[1.5] text-muted">
          {Object.entries(a.data.reasons).map(([k, v]) => (
            <li key={k}>
              · {k}：{v}
            </li>
          ))}
        </ul>
      )}
    </Card>
  )
}
