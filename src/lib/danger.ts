import type { BackupSettings } from './types'

/**
 * 危险操作的「逐字输入」校验（I13 · R4 / R6）。
 *
 * 删除应用要打「删除应用」/「删除应用和数据」，恢复要打「恢复数据」——
 * 三处用的是同一个判据，所以它只能有**一份实现**。
 *
 * 只去掉前后空白，别的一个字都不能差：全角标点、多一个空格、拼音、
 * 大小写不同的英文，一律不算（那几种恰恰是「随手乱按」最容易产生的东西）。
 *
 * Rust 那一侧还会**再核一次**（`uninstall::check_confirm`、`RESTORE_PHRASE`）——
 * 这一份只决定按钮灰不灰，不是安全边界。界面有 bug 也不该把用户的数据删掉。
 */
export function confirmOk(typed: string, phrase: string): boolean {
  return typed.trim() === phrase
}

/**
 * 自动备份开着、而最近一次成功距今超过 25 小时 —— 大概错过了一次
 * （电脑关着 / 睡着，而补跑也没成）。
 *
 * 25 小时而不是 24：备份本身要跑几分钟，卡在整点边上会天天误报。
 * 和 Rust 侧 `schedule::missed` 用的是同一个数，两处必须一致。
 *
 * 时间戳是上海时间的 `YYYY-MM-DD HH:MM:SS`（启动器全局口径）。
 * 认不出来就返回 false —— 宁可不提醒，也不拿一个看不懂的字符串吓唬用户。
 */
export function backupOverdue(b: BackupSettings, now = Date.now()): boolean {
  if (!b.enabled || !b.lastOkAt) return false
  const t = Date.parse(`${b.lastOkAt.replace(' ', 'T')}+08:00`)
  if (Number.isNaN(t)) return false
  return (now - t) / 3_600_000 > 25
}
