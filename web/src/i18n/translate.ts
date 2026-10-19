import type { Locale, Messages } from './types'
import { messages as zh } from './zh'
import { messages as en } from './en'

const dictionaries: Record<Locale, Messages> = { zh, en }

/**
 * Pure translation lookup. Kept in its own module so unit tests can import
 * it without dragging in React (and without adding another fast-refresh
 * "only-export-components" warning to i18n/index.tsx).
 *
 * Fallback order: zh → en → key. The key string is returned unchanged
 * when neither dictionary has the path, which keeps the UI from ever
 * rendering an empty string.
 */
export function translate(
  locale: Locale,
  key: string,
  vars?: Record<string, string | number>,
): string {
  const dict = dictionaries[locale]
  const lookup = (source: Messages): unknown =>
    key.split('.').reduce<unknown>(
      (acc, part) =>
        acc && typeof acc === 'object' ? (acc as Record<string, unknown>)[part] : undefined,
      source,
    )
  const text = lookup(dict)
  if (typeof text === 'string') {
    let result: string = text
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        result = result.replaceAll(`{${k}}`, String(v))
      }
    }
    return result
  }
  const enText = lookup(en)
  return typeof enText === 'string' ? enText : key
}