import { Button, ChevronRight } from '../components/Button'
import { Card } from '../components/Card'
import { CheckCircle } from '../components/Icons'
import { WizardLayout } from '../components/WizardLayout'
import { useAsync } from '../lib/useAsync'
import * as ipc from '../lib/ipc'
import { useStore } from '../state/context'

/** 完成页。方案第 4 节里 Starting 成功后直接进 Ready，这里多插一页，把「打开浏览器」交还给用户。 */
export function Done() {
  const { t, send } = useStore()
  const rt = useAsync(() => ipc.runtimeStatus(), [])
  const url = rt.data?.webUrl ?? null
  // 最后一条提示讲的是「谁能打开这个地址」。I7 起新装的机器一律只有本机能打开，
  // 所以那一条就是默认文案；只有「升级前就对局域网开放、本轮有意没动」的老机器
  // 要换成另一句（照原样显示就成了假话 —— 红线 1）。
  // 依据是 runtimeStatus 里 docker 报的**真实绑定地址**，不是配置。
  const tips = rt.data?.webLanExposed
    ? [...t.done.tips.slice(0, -1), t.done.tipLanExposed]
    : t.done.tips

  return (
    <WizardLayout
      title={t.done.title}
      intro={t.done.intro}
      footerRight={
        <>
          <Button onClick={() => send({ type: 'ENTER_PANEL' })}>{t.done.toPanel}</Button>
          <Button
            variant="primary"
            trailing={<ChevronRight />}
            disabled={!url}
            onClick={() => {
              if (url) void ipc.openExternal(url)
              send({ type: 'ENTER_PANEL' })
            }}
          >
            {t.done.openBrowser}
          </Button>
        </>
      }
    >
      <div className="mt-[30px] flex items-center gap-3">
        <CheckCircle size={22} className="text-success" />
        <span className="tnum text-2xl leading-none text-ink">{url ?? t.app.noData}</span>
      </div>

      <Card className="mt-[26px]">
        <ul className="flex flex-col gap-[11px]">
          {tips.map((tip) => (
            <li key={tip} className="flex gap-2.5 text-md leading-[1.5] text-body">
              <span className="mt-[9px] size-[5px] shrink-0 rounded-full bg-amber" />
              {tip}
            </li>
          ))}
        </ul>
      </Card>
    </WizardLayout>
  )
}
