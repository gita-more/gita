import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from 'react'
import { api, type User } from '../lib/api'
import { translateError } from './errors'
import { translate } from './translate'
import type { Locale } from './types'

// ---------------------------------------------------------------------------
// Module-level locale store (same pattern as useAuth) so the whole app shares
// one source of truth without prop drilling / Context re-render storms.
// ---------------------------------------------------------------------------

function initialLocale(): Locale {
  try {
    const saved = localStorage.getItem('gita_locale')
    if (saved === 'zh' || saved === 'en') return saved
  } catch {
    /* ignore */
  }
  // Fall back to the persisted user object (written on login/register).
  try {
    const savedUser = localStorage.getItem('gita_user')
    if (savedUser) {
      const u = JSON.parse(savedUser) as User
      if (u.locale === 'zh' || u.locale === 'en') return u.locale
    }
  } catch {
    /* ignore */
  }
  return 'zh'
}

let currentLocale: Locale = initialLocale()

const listeners = new Set<() => void>()
function emit() {
  for (const l of listeners) l()
}
function subscribe(cb: () => void) {
  listeners.add(cb)
  return () => {
    listeners.delete(cb)
  }
}
function getSnapshot(): Locale {
  return currentLocale
}

// ---------------------------------------------------------------------------
// Translation (see ./translate.ts)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Setting locale: update memory + localStorage + backend, notify subscribers.
// Backend failure is non-blocking (local switch still applies).
// ---------------------------------------------------------------------------

async function setLocale(locale: Locale): Promise<void> {
  currentLocale = locale
  try {
    localStorage.setItem('gita_locale', locale)
  } catch {
    /* ignore */
  }
  emit()
  try {
    const updated = await api.updateLocale(locale)
    try {
      const savedUser = localStorage.getItem('gita_user')
      if (savedUser) {
        const u = JSON.parse(savedUser) as User
        u.locale = updated.locale
        localStorage.setItem('gita_user', JSON.stringify(u))
      }
    } catch {
      /* ignore */
    }
  } catch {
    // Local switch still applies; the next me() call will resync.
  }
}

// ---------------------------------------------------------------------------
// Error message mapping: backend English messages -> i18n keys.
//
// The mapping itself lives in ./errors.ts so it can be unit-tested without
// React; this module only re-exports it.
// ---------------------------------------------------------------------------

export { translateError }

// ---------------------------------------------------------------------------
// React context / hooks
// ---------------------------------------------------------------------------

export interface I18nValue {
  locale: Locale
  t: (key: string, vars?: Record<string, string | number>) => string
  setLocale: (locale: Locale) => Promise<void>
}

const I18nContext = createContext<I18nValue | null>(null)

export function I18nProvider({ children }: { children: ReactNode }) {
  const locale = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)

  useEffect(() => {
    document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en'
  }, [locale])

  const value = useMemo<I18nValue>(
    () => ({
      locale,
      t: (key, vars) => translate(locale, key, vars),
      setLocale,
    }),
    [locale],
  )

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n(): I18nValue {
  const ctx = useContext(I18nContext)
  if (!ctx) throw new Error('useI18n must be used within I18nProvider')
  return ctx
}

/** Translate an unknown error value (Error.message) into the current locale. */
export function useErrorTranslator() {
  const { locale } = useI18n()
  return useCallback(
    (err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err)
      return translateError(msg, locale)
    },
    [locale],
  )
}