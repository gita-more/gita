import { describe, it, expect } from 'vitest'
import { translateError } from './errors'

/**
 * The backend returns plain English error messages, and the frontend maps
 * them to i18n keys by text. The mapping used to strip the category prefix
 * with a "one word, then a colon" regular expression, which matches none
 * of the prefixes the server actually uses ("not found", "bad request",
 * "rate limited" all contain a space). Every prefixed error therefore
 * reached the UI in English.
 *
 * These tests pin the behaviour that was broken.
 */

describe('translateError()', () => {
  it('translates a bare known message', () => {
    expect(translateError('invalid credentials', 'zh')).toBe('用户名或密码错误')
    expect(translateError('invalid credentials', 'en')).toBe(
      'Incorrect username or password',
    )
  })

  it('translates a message behind a "not found" prefix', () => {
    expect(translateError('not found: repository not found', 'zh')).toBe(
      '仓库不存在',
    )
  })

  it('translates a message behind a "bad request" prefix', () => {
    expect(translateError('bad request: title is required', 'zh')).toBe(
      '标题不能为空',
    )
  })

  it('translates a message behind a "conflict" prefix', () => {
    expect(translateError('conflict: repository already exists', 'zh')).toBe(
      '仓库已存在',
    )
  })

  it('translates a message behind an "unauthorized" prefix', () => {
    expect(translateError('unauthorized: invalid credentials', 'zh')).toBe(
      '用户名或密码错误',
    )
  })

  it('keeps the detail of a known message that carries one', () => {
    // The backend writes "source branch not found: <branch>"; the known
    // message is the part before the colon.
    expect(
      translateError('bad request: source branch not found: feature/x', 'zh'),
    ).toBe('源分支不存在')
  })

  it('translates the category of an unknown message and keeps the detail', () => {
    expect(translateError('not found: some brand new thing', 'zh')).toBe(
      '未找到: some brand new thing',
    )
    expect(translateError('rate limited: slow down', 'en')).toBe(
      'Rate limited: slow down',
    )
  })

  it('returns an unknown message without a category unchanged', () => {
    expect(translateError('something entirely new', 'zh')).toBe(
      'something entirely new',
    )
  })

  it('never leaves the raw "not found" prefix in the output', () => {
    // The 404 page renders t('error.notFound'); that key has to exist in
    // both dictionaries or the page shows the key itself.
    expect(translateError('not found: ', 'zh')).toBe('未找到: ')
  })
})
