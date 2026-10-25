import { translate } from './translate'
import type { Locale } from './types'

/**
 * Backend error messages → i18n keys.
 *
 * The backend returns plain English messages, so the mapping is by
 * message text. It lives in its own module (rather than in `index.tsx`)
 * so it can be unit-tested without dragging in React.
 */
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
  'invalid pages path: expected "/" or "/docs"': 'error.invalidPagesPath',
  'branch not found: no such branch in this repository': 'error.pagesBranchNotFound',
  'only the repository owner can configure pages': 'error.pagesOwnerOnly',
  'pages disabled': 'error.pagesDisabled',
  'admin privileges required': 'error.adminRequired',
  'cannot demote the last administrator: at least one admin must remain': 'error.lastAdmin',
  'cannot delete the last administrator: at least one admin must remain': 'error.lastAdmin',
  'cannot delete your own account': 'error.deleteSelf',
  'username or email already in use': 'error.usernameOrEmailTaken',
}

/**
 * The category prefixes the backend puts in front of an error message,
 * and the i18n key for each category.
 *
 * These are the `#[error("...")]` prefixes of the server's `ApiError`
 * variants. They have to be listed explicitly: the old code stripped
 * "a leading word" with a regular expression that matched a word
 * followed by a colon, which never matched any of them, because
 * "not found", "bad request" and "rate limited" contain a space.
 * Every one of those errors therefore reached the UI untranslated.
 */
const PREFIX_KEYS: Record<string, string> = {
  'bad request': 'error.badRequest',
  unauthorized: 'error.unauthorized',
  forbidden: 'error.forbidden',
  'not found': 'error.notFound',
  conflict: 'error.conflict',
  'rate limited': 'error.rateLimited',
}

/** The longest prefix first, so "not found" wins over a shorter match. */
const PREFIXES: string[] = Object.keys(PREFIX_KEYS).sort(
  (a, b) => b.length - a.length,
)

/**
 * Translate a backend error message into the given locale.
 *
 * The lookup goes:
 *
 * 1. the message as-is;
 * 2. the message with a category prefix removed ("not found: x" → "x");
 * 3. the first colon-separated part of that remainder, which is how the
 *    backend writes "source branch not found: main".
 *
 * If none of those is a known message, the category is still
 * translated and the detail is kept: "未找到: whatever".
 *
 * An unknown message with no category is returned unchanged — inventing
 * a translation for it would be worse than showing the original.
 */
export function translateError(message: string, locale: Locale): string {
  const exact = ERROR_MAP[message]
  if (exact) return translate(locale, exact)

  // Strip a category prefix, if there is one.
  const prefix = PREFIXES.find((p) => message.startsWith(`${p}: `))
  if (prefix) {
    const rest = message.slice(prefix.length + 2)
    // The detail may itself be a known message…
    const mappedRest = ERROR_MAP[rest]
    if (mappedRest) return translate(locale, mappedRest)
    // …or a known message with a trailing detail ("source branch not
    // found: main").
    const mappedHead = ERROR_MAP[rest.split(': ')[0]]
    if (mappedHead) return translate(locale, mappedHead)
    // Nothing known: translate the category and keep the detail.
    return `${translate(locale, PREFIX_KEYS[prefix])}: ${rest}`
  }

  // No prefix, but the message may still carry a trailing detail.
  const head = message.split(': ')[0]
  if (head !== message) {
    const mappedHead = ERROR_MAP[head]
    if (mappedHead) return translate(locale, mappedHead)
  }

  return message
}
