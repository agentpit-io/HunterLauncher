import zhCN, { type Dict } from './zh-CN'
import en from './en'

export type Locale = 'zh-CN' | 'en'
export type { Dict }

export const LOCALES: { id: Locale; label: string }[] = [
  { id: 'zh-CN', label: '简体中文' },
  { id: 'en', label: 'English' },
]

const DICTS: Record<Locale, Dict> = { 'zh-CN': zhCN, en }

export function dictOf(locale: Locale): Dict {
  return DICTS[locale] ?? zhCN
}

/**
 * 从系统语言猜一个默认值。中文系统或识别不出来时都用中文（本项目以中文为主），
 * 明确是其它语言时才预选英文；欢迎页第一件事仍然是让用户自己选。
 */
export function guessLocale(): Locale {
  if (typeof navigator === 'undefined') return 'zh-CN'
  const langs = [navigator.language, ...(navigator.languages ?? [])].filter(Boolean)
  if (langs.length === 0) return 'zh-CN'
  if (langs.some((l) => l.toLowerCase().startsWith('zh'))) return 'zh-CN'
  return 'en'
}
