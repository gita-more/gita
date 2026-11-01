import { describe, it, expect } from 'vitest'
import { relativeTime } from './reltime'

/**
 * Pure-function tests for the GitHub-style relative time used across
 * the repository lists and the activity feeds. No DOM, no React.
 */

const NOW = new Date('2026-11-02T12:00:00Z')

function ago(seconds: number): string {
  return relativeTime(new Date(NOW.getTime() - seconds * 1000).toISOString(), NOW)
}

describe('relativeTime()', () => {
  it('says just now for anything under a minute', () => {
    expect(ago(0)).toBe('just now')
    expect(ago(30)).toBe('just now')
    expect(ago(59)).toBe('just now')
  })

  it('counts minutes', () => {
    expect(ago(60)).toBe('1 minute ago')
    expect(ago(90)).toBe('1 minute ago')
    expect(ago(120)).toBe('2 minutes ago')
    expect(ago(3599)).toBe('59 minutes ago')
  })

  it('counts hours', () => {
    expect(ago(3600)).toBe('1 hour ago')
    expect(ago(7200)).toBe('2 hours ago')
  })

  it('counts days', () => {
    expect(ago(86400)).toBe('1 day ago')
    expect(ago(86400 * 3)).toBe('3 days ago')
  })

  it('counts months', () => {
    expect(ago(86400 * 30)).toBe('1 month ago')
    expect(ago(86400 * 60)).toBe('2 months ago')
  })

  it('counts years', () => {
    expect(ago(86400 * 365)).toBe('1 year ago')
    expect(ago(86400 * 730)).toBe('2 years ago')
  })

  it('does not singularize a zero count', () => {
    // A value of zero is plural: "0 minutes ago", not "0 minute ago".
    expect(ago(0 + 60)).toBe('1 minute ago')
  })

  it('returns the input unchanged when it is not a date', () => {
    expect(relativeTime('not a date', NOW)).toBe('not a date')
  })
})
