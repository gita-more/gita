// Tests for namespace path resolution (src/lib/namespace.ts).
//
// The URL scheme is clean — a group lives at /gita, a repo at /owner/repo,
// a nested group at /gita/tests — which makes the paths ambiguous by shape.
// The server arbitrates (/api/resolve: a repository wins over a group), and
// these tests lock the client side of that contract:
//
//  1. a group path resolves to a group — whatever its depth;
//  2. a repo path resolves to a repo — including one owned by a nested
//     group, where the owner itself spans two segments;
//  3. a repo sub-page (/owner/repo/commits, /owner/repo/settings/general)
//     resolves to the repo plus the sub-page;
//  4. a group never gains a sub-page: /gita/commits is the repo `commits`
//     in group `gita` if it exists, and nothing otherwise;
//  5. a path nothing addresses resolves to null.

import { describe, expect, it } from 'vitest'
import { resolveNamespace, type NamespaceTarget } from './namespace'
import type { ResolvedPath } from './api'

/** A resolver backed by a fixed table, like the server's /api/resolve. */
function resolverFor(table: Record<string, ResolvedPath>) {
  return async (p: string): Promise<ResolvedPath> => {
    const hit = table[p]
    if (!hit) throw new Error('not found')
    return hit
  }
}

function group(fullPath: string): ResolvedPath {
  return { kind: 'group', full_path: fullPath }
}

function repo(owner: string, name: string): ResolvedPath {
  return { kind: 'repo', owner, name, owner_kind: 'group' }
}

async function resolveWith(
  table: Record<string, ResolvedPath>,
  path: string,
): Promise<NamespaceTarget | null> {
  return resolveNamespace(path, resolverFor(table))
}

describe('resolveNamespace', () => {
  it('单段路径解析为群组', async () => {
    const target = await resolveWith({ gita: group('gita') }, 'gita')
    expect(target).toEqual({ kind: 'group', fullPath: 'gita' })
  })

  it('两段路径解析为嵌套群组（仓库不存在时）', async () => {
    const target = await resolveWith({ 'gita/tests': group('gita/tests') }, 'gita/tests')
    expect(target).toEqual({ kind: 'group', fullPath: 'gita/tests' })
  })

  it('两段路径仓库优先于同名群组（服务端规则）', async () => {
    const table = {
      'gita/tests': repo('gita', 'tests'),
      'gita/tests-group': group('gita/tests'),
    }
    const target = await resolveWith(table, 'gita/tests')
    expect(target).toEqual({ kind: 'repo', owner: 'gita', name: 'tests', sub: null })
  })

  it('嵌套群组下的仓库：owner 跨两段', async () => {
    const target = await resolveWith(
      { 'testgroup/sub/nested': repo('testgroup/sub', 'nested') },
      'testgroup/sub/nested',
    )
    expect(target).toEqual({
      kind: 'repo',
      owner: 'testgroup/sub',
      name: 'nested',
      sub: null,
    })
  })

  it('仓库 tab：完整路径解析失败时回退到父命名空间', async () => {
    const target = await resolveWith(
      { 'owner/repo': repo('owner', 'repo') },
      'owner/repo/commits',
    )
    expect(target).toEqual({
      kind: 'repo',
      owner: 'owner',
      name: 'repo',
      sub: { type: 'tab', tab: 'commits' },
    })
  })

  it('仓库设置页：settings/section 回退到父命名空间', async () => {
    const target = await resolveWith(
      { 'owner/repo': repo('owner', 'repo') },
      'owner/repo/settings/general',
    )
    expect(target).toEqual({
      kind: 'repo',
      owner: 'owner',
      name: 'repo',
      sub: { type: 'settings', section: 'general' },
    })
  })

  it('/owner/repo/settings（无 section）也回退到父命名空间', async () => {
    const target = await resolveWith(
      { 'owner/repo': repo('owner', 'repo') },
      'owner/repo/settings',
    )
    expect(target).toEqual({
      kind: 'repo',
      owner: 'owner',
      name: 'repo',
      sub: { type: 'settings', section: null },
    })
  })

  it('群组永远没有子页：/gita/commits 是仓库或不存在，绝不是群组加 tab', async () => {
    // The repo `commits` exists in group `gita` → the URL is that repo.
    const withRepo = await resolveWith(
      { gita: group('gita'), 'gita/commits': repo('gita', 'commits') },
      'gita/commits',
    )
    expect(withRepo).toEqual({ kind: 'repo', owner: 'gita', name: 'commits', sub: null })

    // No repo, no subgroup by that name → nothing addresses the path, even
    // though group `gita` itself exists.
    const without = await resolveWith({ gita: group('gita') }, 'gita/commits')
    expect(without).toBeNull()
  })

  it('什么都不匹配时返回 null', async () => {
    const target = await resolveWith({ gita: group('gita') }, 'nope/nada')
    expect(target).toBeNull()
  })

  it('空路径返回 null', async () => {
    const target = await resolveNamespace('', async () => group('x'))
    expect(target).toBeNull()
  })

  it('子页尾段不是合法 tab 时不回退：/gita/tests 不是群组加 bogus tab', async () => {
    // `tests` is not a repo tab name, so the resolver must not read
    // /gita/tests as group `gita` plus a "tests" sub-page.
    const target = await resolveWith({ gita: group('gita') }, 'gita/tests')
    expect(target).toBeNull()
  })
})
