/*
 * 三级授权（R5 · F-02 的界面那一半）与授权审计里的**纯逻辑**。
 * ---------------------------------------------------------------------------
 * ## 一档是「上限」，不是「单独的一档」
 *
 * Rust 侧 `assist/guard.rs` 的 `GrantSet` 存的是**上限**：`["l1","l2"]`
 * 的意思是「授权到二级」，而 `to_config()` 写回去时会把含在内的低档列全。
 * 所以界面上那三个勾永远是一个**前缀**：
 *
 * ```text
 * 只勾一级 → ["l1"]            勾二级 → ["l1","l2"]        勾三级 → ["l1","l2","l3"]
 * ```
 *
 * 「勾了三级却把一级取消掉」这种事在语义上不存在（三级含一级），
 * 所以 `toggleGrant` 把它规范化回前缀 —— 而不是把一个后端认不得的组合写下去。
 *
 * ## 默认值由后端决定
 *
 * 这个文件**没有默认档位**。默认值在 Rust（`config.rs` 的 `default_grants`：
 * 一级 + 二级，不含三级），界面照读 `settings.assistGrants`。两份默认值迟早会分家。
 */

/** 三档的键。**与配置里的字符串逐字一致**（`Grant::as_str()`）。 */
export type GrantKey = 'l1' | 'l2' | 'l3'

export const GRANT_KEYS: GrantKey[] = ['l1', 'l2', 'l3']

/**
 * 配置里的档位表 → 界面上那三个勾的状态。
 *
 * 认三种写法：`l1`（配置里的正规写法）、`1`（手改配置时可能这么写，
 * Rust 侧 `Grant::parse` 也认）、`observe`/`local`/`elevated`（枚举的序列化名，
 * 万一有别的入口写进来）。认不得的一律忽略 —— **宁可少授权**，与 Rust 同一条。
 */
export function grantsFromConfig(list: string[] | null | undefined): GrantKey[] {
  if (!list) return []
  const alias: Record<string, GrantKey> = {
    l1: 'l1',
    '1': 'l1',
    observe: 'l1',
    l2: 'l2',
    '2': 'l2',
    local: 'l2',
    l3: 'l3',
    '3': 'l3',
    elevated: 'l3',
  }
  const have = new Set<GrantKey>()
  for (const raw of list) {
    const k = alias[String(raw).trim().toLowerCase()]
    if (k) have.add(k)
  }
  return normalize(have)
}

/** 把一组档位规范化成前缀（最高那档含它下面所有档）。 */
export function normalize(keys: Iterable<GrantKey>): GrantKey[] {
  let highest = -1
  for (const k of keys) {
    const i = GRANT_KEYS.indexOf(k)
    if (i > highest) highest = i
  }
  return GRANT_KEYS.slice(0, highest + 1)
}

/**
 * 勾 / 取消一个勾，返回**规范化之后**的档位表。
 *
 * 取消一级 = 一档都不授权（后面几档都是它的超集，不能单独站着）；
 * 取消三级 = 回到二级。这样用户看到的勾永远是一个前缀。
 */
export function toggleGrant(current: GrantKey[], level: GrantKey, on: boolean): GrantKey[] {
  const i = GRANT_KEYS.indexOf(level)
  if (i < 0) return normalize(current)
  return on ? normalize([level]) : normalize(GRANT_KEYS.slice(0, i))
}

/** 档位表 → 写回配置的字符串表（`write_settings` / `assist_consent` 收的就是它）。 */
export function grantsToConfig(keys: GrantKey[]): string[] {
  return [...normalize(keys)]
}

/** 勾没勾某一档。 */
export function hasGrant(keys: GrantKey[], level: GrantKey): boolean {
  return keys.includes(level)
}

// ── 授权审计 ──────────────────────────────────────────────────────────────

/**
 * `assist_audit_tail` 里的一行（Rust 侧 `guard::audit` 手写的 JSON）。
 *
 * 字段是 **snake_case**（那一处是手写 `json!`，不受 rename 影响）：
 * `{"at":"2026-09-21 11:02:44","action":"consent","args":{…},"by":"user","level":null,"result":"…"}`
 */
export interface AuditEntry {
  /** 上海时间，Rust 写的时候就格式化好了 */
  at: string
  action: string
  /** `rule` / `model` / `orchestrator` / `user` */
  by: string
  /** Rust 写下的那句人话 —— 界面上直接显示它，不再自己拼一遍 */
  result: string
  /** 这一次是不是改了授权 */
  isGrantChange: boolean
  /** 改成了哪几档（不是授权变更时是 `null`） */
  grants: GrantKey[] | null
}

/**
 * 一行 JSON → 一条记录。**解不开就返回 `null`**，由界面原样显示那一行 ——
 * 审计日志里少写一行、或者格式变过，都不该让整块面板空掉。
 */
export function parseAuditLine(line: string): AuditEntry | null {
  const text = line.trim()
  if (text === '') return null
  let raw: unknown
  try {
    raw = JSON.parse(text)
  } catch {
    return null
  }
  if (raw === null || typeof raw !== 'object') return null
  const o = raw as Record<string, unknown>
  const args = (o.args ?? {}) as Record<string, unknown>
  const grantsRaw = typeof args.grants === 'string' ? args.grants.split(',') : null
  const grants = grantsRaw ? grantsFromConfig(grantsRaw) : null
  return {
    at: typeof o.at === 'string' ? o.at : '',
    action: typeof o.action === 'string' ? o.action : '',
    by: typeof o.by === 'string' ? o.by : '',
    result: typeof o.result === 'string' ? o.result : '',
    // 只有**带 grants 的 consent** 才算改了授权。
    // 不带 grants 的 consent 是「授权页上点了开始」（args 里没有档位），
    // 把它也算成「改了授权」的话，界面上会到处是假的变更记录
    isGrantChange: o.action === 'consent' && grants !== null && grants.length > 0,
    grants,
  }
}

/** 一批审计行 → 记录表；解不开的行原样留在 `raw` 里。 */
export function parseAudit(lines: string[]): { entries: AuditEntry[]; raw: string[] } {
  const entries: AuditEntry[] = []
  const raw: string[] = []
  for (const line of lines) {
    const e = parseAuditLine(line)
    if (e) entries.push(e)
    else if (line.trim() !== '') raw.push(line)
  }
  return { entries, raw }
}

/** 最近一次改授权是哪一条（给「现在的授权是什么时候定的」那行用）。 */
export function lastGrantChange(entries: AuditEntry[]): AuditEntry | null {
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const e = entries[i]!
    if (e.isGrantChange) return e
  }
  return null
}
