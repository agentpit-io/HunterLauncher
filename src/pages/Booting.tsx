import { CandlestickDecor } from '../components/CandlestickDecor'
import { useStore } from '../state/context'

/**
 * 「正在检查 Hunter 状态」（I12 · R1 · 方案第三节第 1 条）。
 *
 * ## 这一页存在的唯一理由
 *
 * 0.1.11 及之前，启动器一起来就渲染欢迎页，`boot_state` 是异步回来的 ——
 * 于是一台早就装好、六个容器正健康跑着的机器，打开启动器会**先闪一下**
 * 「欢迎使用 Hunter 启动器」再跳到运行面板。用户 2026-09-22 23:10 的截图
 * 拍到的就是那一帧（而且那一次因为 `install.done` 是 false，它压根没跳走）。
 *
 * 所以这一页要做的事只有一件：**在拿到判定结果之前，不要说任何可能是错的话。**
 * 它不说「还没装」，也不说「已经装好了」——只说「正在检查」。
 *
 * 按方案要求，这一页停留**不超过 1 秒**（后端那次复查是浅查：一次
 * `docker compose ps` + 一次本机 GET，实测几百毫秒）。查完由 `StoreProvider`
 * 按后端给的 `route` 跳到三条路之一，谁都不会看到向导闪一下。
 */
export function Booting() {
  const { t } = useStore()
  return (
    <section
      className="relative flex min-h-0 flex-1 flex-col items-center justify-center bg-window px-panel"
      data-testid="booting"
    >
      <CandlestickDecor className="absolute bottom-[26px] left-[26px] opacity-60" />
      <div className="flex flex-col items-center gap-[18px]">
        {/* 琥珀金的脉冲圆环，和过程流里「进行中」那张卡片同一套语言 */}
        <span className="relative flex size-[46px] items-center justify-center">
          <span className="absolute inset-0 animate-ping rounded-full border border-amber/40" />
          <span className="absolute inset-[9px] rounded-full border-2 border-amber/70" />
          <span className="size-[8px] rounded-full bg-amber" />
        </span>
        <h1 className="text-2xl font-semibold leading-none text-ink">{t.booting.title}</h1>
        <p className="max-w-[460px] text-center text-md leading-[1.55] text-muted">
          {t.booting.body}
        </p>
      </div>
    </section>
  )
}
