/**
 * 左下角的 K 线装饰。
 * 十三根蜡烛的位置、影线与实体范围全部量自视觉稿第 1 张（脚本 scripts/sample-mockup.py），
 * 单位是逻辑像素（视觉稿 2 倍图的测量值 ÷2）。第 10 根是金色的那根。
 */

interface Candle {
  cx: number
  top: number
  bot: number
  bodyTop: number
  bodyBot: number
}

const CANDLES: Candle[] = [
  { cx: 4.8, top: 44, bot: 104, bodyTop: 61.5, bodyBot: 88.5 },
  { cx: 22.2, top: 54, bot: 114, bodyTop: 75.5, bodyBot: 100.5 },
  { cx: 40.8, top: 34, bot: 96, bodyTop: 47.5, bodyBot: 78.5 },
  { cx: 58.5, top: 28, bot: 82, bodyTop: 35.5, bodyBot: 66.5 },
  { cx: 76.8, top: 40, bot: 92, bodyTop: 53.5, bodyBot: 76.5 },
  { cx: 94.5, top: 24, bot: 80, bodyTop: 31.5, bodyBot: 66.5 },
  { cx: 112.5, top: 14, bot: 68, bodyTop: 21.5, bodyBot: 50.5 },
  { cx: 130.2, top: 32, bot: 84, bodyTop: 45.5, bodyBot: 68.5 },
  { cx: 148.5, top: 18, bot: 74, bodyTop: 27.5, bodyBot: 58.5 },
  { cx: 166.8, top: 4, bot: 62, bodyTop: 11.5, bodyBot: 46.5 },
  { cx: 184.5, top: 20, bot: 72, bodyTop: 33.5, bodyBot: 56.5 },
  { cx: 202.5, top: 10, bot: 64, bodyTop: 17.5, bodyBot: 48.5 },
  { cx: 220.8, top: 24, bot: 78, bodyTop: 39.5, bodyBot: 62.5 },
]

const GOLD_INDEX = 9
const BODY_W = 9.5
const WICK_W = 1.5
const W = 226
const H = 116

export function CandlestickDecor({ className = '' }: { className?: string }) {
  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      width={W}
      height={H}
      aria-hidden
      focusable="false"
      className={`pointer-events-none select-none ${className}`}
    >
      {CANDLES.map((c, i) => {
        const gold = i === GOLD_INDEX
        const fill = gold ? 'var(--hl-amber-candle)' : 'var(--hl-candle)'
        return (
          <g key={i} fill={fill}>
            <rect x={c.cx - WICK_W / 2} y={c.top} width={WICK_W} height={c.bot - c.top} rx={WICK_W / 2} />
            <rect x={c.cx - BODY_W / 2} y={c.bodyTop} width={BODY_W} height={c.bodyBot - c.bodyTop} rx={1} />
          </g>
        )
      })}
    </svg>
  )
}
