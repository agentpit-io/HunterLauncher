import { describe, expect, it } from 'vitest'
import { backupOverdue, confirmOk } from './danger'
import type { BackupSettings } from './types'

/**
 * 方案第七节用例 6：**输入框填错一个字，确认按钮不可点。**
 *
 * 这条在界面上的表现就是 `disabled={!confirmOk(typed, phrase)}`，
 * 所以判据本身必须有测试 —— 它是那个用例的全部内容。
 */
describe('危险操作的逐字输入', () => {
  it('一字不差才放行，前后空白不算', () => {
    expect(confirmOk('删除应用', '删除应用')).toBe(true)
    expect(confirmOk('  删除应用 ', '删除应用')).toBe(true)
    expect(confirmOk('删除应用和数据', '删除应用和数据')).toBe(true)
    expect(confirmOk('恢复数据', '恢复数据')).toBe(true)
  })

  it('差一个字就不放行', () => {
    for (const bad of [
      '删除应', // 少一个
      '删除应用。', // 多一个句号
      '删除引用', // 错一个字
      '删除 应用', // 中间多一个空格
      '删除应用和数据', // 打的是另一档那一句
      'shanchuyingyong',
      '',
      '删除应用!',
    ]) {
      expect(confirmOk(bad, '删除应用'), `「${bad}」不该被当成「删除应用」`).toBe(false)
    }
  })

  it('两档要打的句子不一样，短的那句不能顶替长的', () => {
    expect(confirmOk('删除应用', '删除应用和数据')).toBe(false)
  })
})

describe('自动备份好像错过了一次', () => {
  const base: BackupSettings = {
    enabled: true,
    time: '00:00',
    dir: '',
    effectiveDir: '/home/u/Hunter-backups',
    keepDays: 3,
    includeSessions: true,
    includeSkills: true,
    lastOkAt: '',
    lastError: '',
    lastRunAt: '',
    failStreak: 0,
    count: 0,
    totalBytes: 0,
    diskFreeBytes: null,
    suggestedDir: '/home/u/Hunter-backups',
    externalSuggestions: [],
  }
  // 2026-09-23 12:00:00 +08:00
  const now = Date.parse('2026-09-23T12:00:00+08:00')

  it('刚备份过不算错过', () => {
    expect(backupOverdue({ ...base, lastOkAt: '2026-09-23 00:00:00' }, now)).toBe(false)
  })

  it('超过 25 小时才算错过（24 小时整不算，备份本身要跑几分钟）', () => {
    expect(backupOverdue({ ...base, lastOkAt: '2026-09-22 12:30:00' }, now)).toBe(false)
    expect(backupOverdue({ ...base, lastOkAt: '2026-09-22 10:00:00' }, now)).toBe(true)
  })

  it('关掉自动备份就不提醒', () => {
    expect(backupOverdue({ ...base, enabled: false, lastOkAt: '2026-09-01 00:00:00' }, now)).toBe(false)
  })

  it('从来没成功过不提醒（那是另一件事，由 lastError 那条横幅说）', () => {
    expect(backupOverdue({ ...base, lastOkAt: '' }, now)).toBe(false)
  })

  it('认不出来的时间戳不提醒，不拿看不懂的字符串吓唬用户', () => {
    expect(backupOverdue({ ...base, lastOkAt: '昨天' }, now)).toBe(false)
  })
})
