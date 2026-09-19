import { useState } from 'react'
import { Button } from '../components/Button'
import { LogBox } from '../components/LogBox'
import { SegmentedControl } from '../components/Field'
import { PlainLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { redact } from '../lib/mask'
import { useStore } from '../state/context'

const TAIL = 200

export function Logs() {
  const { t, setOverlay } = useStore()
  const [source, setSource] = useState<'launcher' | 'compose'>('launcher')
  const log = useAsync(() => (source === 'launcher' ? ipc.launcherLog(TAIL) : ipc.composeLogs(undefined, TAIL)), [source])
  // 红线 2：复制出去的内容一定先脱敏
  const lines = (log.data ?? []).map(redact)

  return (
    <PlainLayout
      title={t.logs.title}
      right={
        <>
          <SegmentedControl<'launcher' | 'compose'>
            value={source}
            onChange={setSource}
            options={[
              { id: 'launcher', label: t.logs.sourceLauncher },
              { id: 'compose', label: t.logs.sourceCompose },
            ]}
          />
          <Button size="sm" onClick={() => void navigator.clipboard?.writeText(lines.join('\n'))}>
            {t.logs.copyAll}
          </Button>
          <Button size="sm" onClick={() => setOverlay(null)}>
            {t.common.close}
          </Button>
        </>
      }
      footer={
        <>
          <span className="text-sm text-muted">{t.logs.redactedNote}</span>
          <span className="tnum text-sm text-muted">{t.logs.tail(TAIL)}</span>
        </>
      }
    >
      <LogBox
        lines={lines}
        className="h-full"
        emptyText={log.error ? `${log.error.code} · ${log.error.message}` : t.logs.empty}
      />
    </PlainLayout>
  )
}
