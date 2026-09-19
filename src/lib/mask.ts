/*
 * 打码与脱敏（总控规则红线 2 / 技术方案 12.3）
 * hunter key 只在输入框里以明文存在；一旦要展示、复制、写进日志或诊断包，都要先过这里。
 */

/** 真实 key 的形状：hunt_tools_ 前缀 + 32 位随机，合计 43 字符（M0 §1.1 实测）。 */
export const KEY_PREFIX = 'hunt_tools_'
export const KEY_LENGTH = 43

export function isKeyShape(key: string): boolean {
  return key.startsWith(KEY_PREFIX) && key.length === KEY_LENGTH
}

/**
 * 输入框里显示用的打码：保留前 head 个字符与后 tail 个字符，中间换成圆点。
 * 视觉稿第 1 张就是这个样子（前缀 + 一串圆点 + 末 4 位）。
 */
export function maskKey(key: string, head = 19, tail = 4, dot = '•'): string {
  if (!key) return ''
  if (key.length <= head + tail) return key
  return key.slice(0, head) + dot.repeat(Math.max(4, key.length - head - tail)) + key.slice(-tail)
}

/** 写进文档、日志、报告里的极简形式。 */
export function maskKeyShort(key: string): string {
  if (!key) return ''
  return `${KEY_PREFIX}****`
}

const REDACTIONS: [RegExp, string][] = [
  [/hunt_tools_[A-Za-z0-9_-]{4,}/g, 'hunt_tools_****'],
  [/\bsk-[A-Za-z0-9_-]{8,}/g, 'sk-****'],
  [/\bhk_[A-Za-z0-9_-]{8,}/g, 'hk_****'],
  [/\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/gi, 'Bearer ****'],
  [/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g, '****@****'],
  [/\b1[3-9]\d{9}\b/g, '1**********'],
  [/\b(\d{1,3}\.\d{1,3}\.\d{1,3})\.\d{1,3}\b/g, '$1.x'],
]

/**
 * 日志 / 诊断包脱敏。规则来自技术方案 12.3，顺序有讲究：
 * 先去掉各种 key（它们可能出现在 Bearer 后面），再处理邮箱、手机号、IP。
 */
export function redact(text: string): string {
  return REDACTIONS.reduce((acc, [re, to]) => acc.replace(re, to), text)
}
