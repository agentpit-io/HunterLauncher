import { describe, expect, it } from 'vitest'
import { KEY_LENGTH, KEY_PREFIX, isKeyShape, maskKey, maskKeyShort, redact } from './mask'

// 测试里出现的都是**编造的**假 key，不是测试机上那把真 key（总控规则红线 2）
const FAKE_KEY = `${KEY_PREFIX}9f2c7a41aaaaaaaaaaaaaaaaaaaab3e1`

describe('key 形状', () => {
  it('真实形状是 hunt_tools_ 开头的 43 位', () => {
    expect(FAKE_KEY).toHaveLength(KEY_LENGTH)
    expect(isKeyShape(FAKE_KEY)).toBe(true)
  })

  it('前缀不对或长度不对都判不合格', () => {
    expect(isKeyShape('hk_9f2c7a41b3e1')).toBe(false)
    expect(isKeyShape(`${KEY_PREFIX}short`)).toBe(false)
    expect(isKeyShape('')).toBe(false)
  })
})

describe('打码', () => {
  it('保留前缀与末 4 位，中段全是圆点', () => {
    const masked = maskKey(FAKE_KEY)
    expect(masked.startsWith(`${KEY_PREFIX}9f2c7a41`)).toBe(true)
    expect(masked.endsWith('b3e1')).toBe(true)
    expect(masked).toContain('•')
    expect(masked).not.toContain('aaaa')
  })

  it('长度与原串一致，界面不会跳宽', () => {
    expect(maskKey(FAKE_KEY)).toHaveLength(FAKE_KEY.length)
  })

  it('太短的串原样返回，不会因为切片露出更多', () => {
    expect(maskKey('abc')).toBe('abc')
    expect(maskKey('')).toBe('')
  })

  it('写进文档的极简形式只剩前缀', () => {
    expect(maskKeyShort(FAKE_KEY)).toBe('hunt_tools_****')
  })
})

describe('脱敏', () => {
  it('抹掉 hunter key', () => {
    expect(redact(`LLM_API_KEY=${FAKE_KEY}`)).toBe('LLM_API_KEY=hunt_tools_****')
  })

  it('抹掉厂商 key 与 Bearer', () => {
    expect(redact('sk-abcdefgh12345678')).toBe('sk-****')
    expect(redact('Authorization: Bearer eyJhbGciOi.JIUzI1')).toBe('Authorization: Bearer ****')
  })

  it('抹掉邮箱与手机号', () => {
    expect(redact('联系 someone@example.com')).toBe('联系 ****@****')
    expect(redact('手机 13812345678')).toBe('手机 1**********')
  })

  it('IP 只留前三段', () => {
    expect(redact('client 192.168.31.77 connected')).toBe('client 192.168.31.x connected')
  })

  it('一行里有多个敏感串时全部处理', () => {
    const line = `key=${FAKE_KEY} ip=10.0.0.9 mail=a@b.cn`
    expect(redact(line)).toBe('key=hunt_tools_**** ip=10.0.0.x mail=****@****')
  })

  it('没有敏感内容时原样返回', () => {
    const line = '17:42:08 pull hunter-api layer 7/12 sha256:4c1e… extracting'
    expect(redact(line)).toBe(line)
  })
})
