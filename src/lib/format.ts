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

/**
 * 网关的 `reset_at`（ISO 8601，例如 `2026-09-21T00:00:00+08:00`）→ 「2026-09-21 00:00（上海）」。
 *
 * **不做时区换算**：网关自己在 `reset_tz` 里写明了是 `Asia/Shanghai`，这里唯一要传达的
 * 信息是「明天 0 点」，为此拖一个时区库不划算。规则与 Rust 侧的 `gateway::pretty_reset`
 * 保持一致（两边各有一份测试盯着同样的几个样本）。
 *
 * 认不出来的格式一律返回 null —— 宁可界面上少一句话，也不猜一个时间出来（红线 1）。
 */
export function shanghaiStamp(iso: string | null | undefined): string | null {
  if (!iso) return null
  const i = iso.indexOf('T')
  if (i !== 10) return null
  const date = iso.slice(0, 10)
  const hm = iso.slice(11, 16)
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date) || !/^\d{2}:\d{2}$/.test(hm)) return null
  return `${date} ${hm}（上海）`
}

/**
 * GitHub / 网关给的 UTC 时间戳（`2026-09-19T09:47:29Z`）→ 上海时间
 * 「2026-09-19 17:47（上海）」。
 *
 * 总控规则：时间一律上海时间。更新页原来把 GitHub Release 的 `published_at`
 * **原样**上屏（带 `T` 和 `Z`），既不好读，也和界面上别处的时间不是一套（I3 自审）。
 *
 * 这里是真的做换算（不像 {@link shanghaiStamp} —— 那个的来源网关自己就写明了是上海时间）：
 * 带时区的 ISO 交给 `Date` 解析，再按固定的 +08:00 偏移格式化。
 * 不用 `toLocaleString('zh-CN', {timeZone})` —— WebKitGTK 上的 ICU 数据未必带得全，
 * 而 +08:00 是个常数，自己加靠得住。解析不出来的一律原样返回，不猜。
 */
export function isoToShanghai(iso: string): string {
  const ms = Date.parse(iso)
  if (!Number.isFinite(ms)) return iso
  const d = new Date(ms + 8 * 3600 * 1000)
  const p = (n: number) => String(n).padStart(2, '0')
  return (
    `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ` +
    `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}（上海）`
  )
}
