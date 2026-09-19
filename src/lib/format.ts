/** 数字、字节、时长的显示格式。界面上所有数值都走这里，保证三张页面的写法一致。 */

/** 千分位。视觉稿里的 300,000 / 42,300 都是这个格式。 */
export function thousands(n: number): string {
  return Math.round(n).toLocaleString('en-US')
}

/**
 * 字节转人类可读。用 SI 单位（1 MB = 10^6 B），与 docker / docker compose 打印的一致。
 * M0 报告里的镜像大小是 MiB，换算关系写在 docs/design/设计令牌.md 与 M1 报告里。
 */
export function bytes(n: number, digits?: number): string {
  if (!Number.isFinite(n) || n < 0) return '—'
  if (n < 1000) return `${Math.round(n)} B`
  const units = ['kB', 'MB', 'GB', 'TB']
  let v = n / 1000
  let i = 0
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000
    i += 1
  }
  const d = digits ?? (v >= 100 ? 0 : v >= 10 ? 0 : 2)
  return `${v.toFixed(d)} ${units[i]}`
}

/** 百分比，整数，夹在 0–100。 */
export function percent(done: number, total: number): number {
  if (!Number.isFinite(total) || total <= 0) return 0
  return Math.max(0, Math.min(100, Math.round((done / total) * 100)))
}

/** 秒 → 「1 分 20 秒」/「45 秒」/「2 小时 3 分」。 */
export function duration(sec: number, locale: 'zh-CN' | 'en' = 'zh-CN'): string {
  if (!Number.isFinite(sec) || sec < 0) return '—'
  const s = Math.round(sec)
  const zh = locale === 'zh-CN'
  if (s < 60) return zh ? `${s} 秒` : `${s}s`
  const m = Math.floor(s / 60)
  const rs = s % 60
  if (m < 60) {
    if (rs === 0) return zh ? `${m} 分` : `${m}m`
    return zh ? `${m} 分 ${rs} 秒` : `${m}m ${rs}s`
  }
  const h = Math.floor(m / 60)
  const rm = m % 60
  if (h < 24) {
    if (rm === 0) return zh ? `${h} 小时` : `${h}h`
    return zh ? `${h} 小时 ${rm} 分` : `${h}h ${rm}m`
  }
  const d = Math.floor(h / 24)
  const rh = h % 24
  if (rh === 0) return zh ? `${d} 天` : `${d}d`
  return zh ? `${d} 天 ${rh} 小时` : `${d}d ${rh}h`
}
