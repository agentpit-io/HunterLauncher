import { useState } from 'react'
import { Badge } from '../components/Badge'
import { Button, ChevronRight } from '../components/Button'
import { Card, CardHead } from '../components/Card'
import { WizardLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { bytes } from '../lib/format'
import { useStore } from '../state/context'
import type { DataCheck } from '../lib/types'

/**
 * 「检测到上次的数据」（I12 · R5 · R1 五行表的第四行）。
 *
 * 出现的条件很窄：**compose 项目 `hunter` 下一个容器都没有，但它的数据卷还在**。
 * 最常见的来路是用户（或以后 I13 的「删除应用 ①」）删掉了容器、留下了数据。
 *
 * 这一页只做两件事：
 *
 * 1. 把**实测到的东西**摆出来 —— 有哪几个卷、数据库多大、密钥卷在不在、
 *    `.env` 里的 `JWT_SECRET` 还在不在。全部来自后端的 `data_check`（只读，
 *    一个字节都不写）。想看得更细就点「看看里面有什么」，那一下才会起一个
 *    用完就删的一次性 postgres 去数表（十几秒，所以不默认做）。
 * 2. 给一条明确的路：继续安装，**沿用这些数据**。
 *
 * 界面上不出现任何「请在终端执行」。删除数据这件事**不在这一页上** ——
 * 那是 I13 的 R4，要走「逐字输入确认」那一套。
 */
export function DataFound() {
  const { t, send } = useStore()
  const shallow = useAsync(() => ipc.dataCheck(), [])
  const [deep, setDeep] = useState<DataCheck | null>(null)
  const [digging, setDigging] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const d = deep ?? shallow.data

  async function dig() {
    setDigging(true)
    setErr(null)
    try {
      setDeep(await ipc.dataCheckDeep())
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setDigging(false)
    }
  }

  return (
    <WizardLayout
      title={t.dataFound.title}
      intro={t.dataFound.intro}
      footerRight={
        <Button
          variant="primary"
          trailing={<ChevronRight />}
          data-testid="data-found-continue"
          onClick={() => send({ type: 'DATA_CONTINUE' })}
        >
          {t.dataFound.continue}
        </Button>
      }
    >
      <div className="mt-[26px] flex flex-col gap-gap">
        <Card>
          <CardHead
            title={t.dataFound.summary}
            right={<Badge tone="amber">{t.common.measured}</Badge>}
          />
          <p className="mt-[12px] text-md leading-[1.55] text-body" data-testid="data-found-headline">
            {d ? d.headline : shallow.loading ? t.common.loading : (shallow.error?.message ?? t.app.noDataReason)}
          </p>
          {d && (
            <ul className="mt-[14px] flex flex-col gap-[8px]">
              <li className="text-sm text-body">
                · {t.dataFound.jwt}：
                <span className={d.hasJwtSecret ? 'text-ok' : 'text-danger'}>
                  {d.hasJwtSecret ? t.dataFound.jwtKept : t.dataFound.jwtGone}
                </span>
              </li>
              <li className="text-sm text-body">
                · {t.dataFound.secrets}：
                <span className={d.hasSecrets ? 'text-ok' : 'text-danger'}>
                  {d.hasSecrets ? t.common.yes : t.dataFound.secretsGone}
                </span>
              </li>
              {/* PG 大版本：两边一致才说得上「能直接沿用」。拿不到就显示「—」 */}
              <li className="tnum text-sm text-body">
                · {t.dataFound.pgVersion}：{d.pgVersion ?? t.app.noData}
                {d.targetPgVersion ? ` → ${d.targetPgVersion}` : ''}
              </li>
              {d.backups > 0 && (
                <li className="tnum text-sm text-body">· {t.dataFound.backups(d.backups)}</li>
              )}
            </ul>
          )}
          {d?.deepSkipped && (
            <p className="mt-[12px] text-xs leading-[1.5] text-muted">{d.deepSkipped}</p>
          )}
        </Card>

        <Card>
          <CardHead
            title={t.dataFound.volumes}
            right={
              <Button size="sm" disabled={digging || !d} data-testid="data-found-dig" onClick={() => void dig()}>
                {digging ? t.dataFound.digging : t.dataFound.dig}
              </Button>
            }
          />
          <div className="mt-[14px] flex flex-col gap-[8px]">
            {(d?.volumes ?? []).map((v) => (
              <div key={v.name} className="flex items-baseline justify-between gap-4">
                <span className="tnum min-w-0 truncate text-sm text-body">{v.short}</span>
                <span className="tnum shrink-0 text-sm text-muted">
                  {v.sizeBytes === null ? t.app.noData : bytes(v.sizeBytes)}
                </span>
              </div>
            ))}
            {d && d.volumes.length === 0 && (
              <div className="text-sm text-muted">{t.dataFound.noVolumes}</div>
            )}
          </div>
          {d?.deep && (
            <div className="tnum mt-[14px] border-t border-line pt-[12px] text-sm text-body">
              {t.dataFound.deepLine(d.tableCount, d.lastWrite, d.migrationMax)}
            </div>
          )}
          {err && <div className="mt-[12px] text-sm text-danger">{err}</div>}
        </Card>

        {d && (
          <details className="rounded-lg border border-line bg-card px-card py-[14px]">
            <summary className="cursor-pointer select-none text-sm text-muted">
              {t.common.evidence}
            </summary>
            <ul className="tnum mt-[12px] flex flex-col gap-[6px] text-xs leading-[1.5] text-dim">
              {d.lines.map((l, i) => (
                <li key={`${i}-${l}`} className="break-all">
                  · {l}
                </li>
              ))}
            </ul>
          </details>
        )}
      </div>
    </WizardLayout>
  )
}
