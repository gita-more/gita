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
import { messages as zh } from './zh'
import { messages as en } from './en'
import type { Locale, Messages } from './types'

const dictionaries: Record<Locale, Messages> = { zh, en }

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
// Translation
// ---------------------------------------------------------------------------

function translate(locale: Locale, key: string, vars?: Record<string, string | number>): string {
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
  // Fallback: English, then the key itself.
  const enText = lookup(en)
  return typeof enText === 'string' ? enText : key
}

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
// ---------------------------------------------------------------------------

const ERROR_MAP: Record<string, string> = {
  'username already taken': 'error.usernameTaken',
  'email already registered': 'error.emailRegistered',
  'invalid credentials': 'error.invalidCredentials',
  'user not found': 'error.userNotFound',
  'username must be at least 2 characters': 'error.usernameTooShort',
  'invalid email': 'error.invalidEmail',
  'password must be at least 6 characters': 'error.passwordTooShort',
  'invalid locale': 'error.invalidLocale',
  'missing Authorization header': 'error.missingAuth',
  'expected Bearer token': 'error.expectedBearer',
  'user no longer exists': 'error.userNoLongerExists',
  'repository not found': 'error.repoNotFound',
  'file not found': 'error.fileNotFound',
  'group not found': 'error.groupNotFound',
  'issue not found': 'error.issueNotFound',
  'pull request not found': 'error.prNotFound',
  'repository already exists': 'error.repoAlreadyExists',
  'group already exists': 'error.groupAlreadyExists',
  'issue already closed': 'error.issueAlreadyClosed',
  'pull request is not open': 'error.prNotOpen',
  'title is required': 'error.titleRequired',
  'invalid status': 'error.invalidStatus',
  "missing 'path' query": 'error.missingPath',
  "repository name may only contain letters, numbers, '-', '_', '.'": 'error.repoNameInvalid',
  "group name may only contain letters, numbers, '-', '_', '.'": 'error.groupNameInvalid',
  'only the owner can delete this repo': 'error.repoOwnerOnly',
  'only the group owner can create repositories': 'error.groupOwnerOnly',
  'source branch not found': 'error.sourceBranchNotFound',
  'target branch not found': 'error.targetBranchNotFound',
  'repo must be in owner/name format': 'error.repoFormat',
}

export function translateError(message: string, locale: Locale): string {
  // Try the raw message first, then a common pattern: the backend
  // prefixes errors with their HTTP status word ("unauthorized: invalid
  // credentials"), so strip a leading "word: " and try again.
  const key = ERROR_MAP[message] ?? ERROR_MAP[message.replace(/^\w+:\s*/, '')]
  if (key) return translate(locale, key)
  return message
}

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