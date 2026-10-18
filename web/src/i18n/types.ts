export type Locale = 'zh' | 'en'

// Derive the message key type from the zh dictionary so en.ts must be
// structurally identical to zh.ts (TypeScript will error otherwise).
import type { messages as zhMessages } from './zh'

export type Messages = typeof zhMessages
export type MessageKey = keyof typeof zhMessages