// @vitest-environment jsdom
// CloneDropdown and ReadmePanel render into a DOM, so these tests need
// one; the pure i18n tests stay in plain Node.
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

// RTL's automatic cleanup relies on global afterEach hooks, which only
// exist when vitest `globals` is on. Clean up explicitly so one test's
// DOM does not leak into the next.
afterEach(cleanup)
import { I18nProvider } from '../i18n'
import { CloneDropdown } from './CloneDropdown'
import { buildCloneUrl } from './EmptyRepoWelcome'
import { ReadmePanel } from './ReadmePanel'

// ---------------------------------------------------------------------------
// buildCloneUrl — the URL shape the UI shows and the E2E suite relies on
// ---------------------------------------------------------------------------

describe('buildCloneUrl', () => {
  it('个人仓的 clone URL 是 /<owner>/<repo>.git，没有 /git 前缀', () => {
    expect(buildCloneUrl('alice', 'widgets', 'personal')).toBe(
      'http://localhost:3000/alice/widgets.git',
    )
  })

  it('群组仓用群组完整路径做命名空间', () => {
    expect(buildCloneUrl('someone', 'widgets', 'group', 'groupa/subgroupa')).toBe(
      'http://localhost:3000/groupa/subgroupa/widgets.git',
    )
  })

  it('群组仓缺 groupName 时回落到 owner 路径', () => {
    expect(buildCloneUrl('alice', 'widgets', 'group')).toBe(
      'http://localhost:3000/alice/widgets.git',
    )
  })
})

// ---------------------------------------------------------------------------
// ReadmePanel — markdown 渲染 + DOMPurify 消毒
// ---------------------------------------------------------------------------

// The panel fetches the README through the api module; replace it so the
// tests control what the "file" contains.
vi.mock('../lib/api', () => ({
  api: {
    getFile: vi.fn(),
  },
}))

import { api } from '../lib/api'

const getFile = vi.mocked(api.getFile)

function renderReadme(content: string | null) {
  getFile.mockImplementation(() =>
    content === null
      ? Promise.reject(new Error('missing'))
      : Promise.resolve({ path: 'README.md', content, sha: 'abc' }),
  )
  return render(
    <I18nProvider>
      <ReadmePanel owner="alice" name="widgets" readmePath="README.md" rev="main" />
    </I18nProvider>,
  )
}

describe('ReadmePanel', () => {
  it('渲染 README 的 markdown', async () => {
    const { container } = renderReadme('# Hello\n\nsome *text*')
    await waitFor(() => {
      expect(container.querySelector('h1')?.textContent).toBe('Hello')
    })
    expect(container.querySelector('em')?.textContent).toBe('text')
  })

  it('剥离 markdown 里的 script 标签', async () => {
    const { container } = renderReadme('# Hi\n\n<script>window.evil = true</script>')
    await waitFor(() => {
      expect(container.querySelector('h1')?.textContent).toBe('Hi')
    })
    expect(container.querySelector('script')).toBeNull()
  })

  it('剥离内联事件处理器（onerror 等）', async () => {
    const { container } = renderReadme(
      '![x](https://example.com/x.png "onerror=alert(1)") <img src=x onerror="alert(1)">',
    )
    await waitFor(() => {
      expect(container.querySelector('img')).not.toBeNull()
    })
    const img = container.querySelector('img')!
    expect(img.getAttribute('onerror')).toBeNull()
  })

  it('没有 README 时什么都不渲染', () => {
    const { container } = render(
      <I18nProvider>
        <ReadmePanel owner="alice" name="widgets" readmePath={null} rev="main" />
      </I18nProvider>,
    )
    expect(container.querySelector('.readme-box')).toBeNull()
    expect(getFile).not.toHaveBeenCalled()
  })
})

// ---------------------------------------------------------------------------
// CloneDropdown — 协议切换与 URL 展示
// ---------------------------------------------------------------------------

describe('CloneDropdown', () => {
  it('展开后显示 HTTPS clone URL', async () => {
    const { user } = setup(
      <CloneDropdown owner="alice" name="widgets" repoData={null} />,
    )
    await user.click(screen.getByRole('button', { name: /克隆|Clone/ }))
    const input = screen.getByRole('textbox') as HTMLInputElement
    expect(input.value).toBe('http://localhost:3000/alice/widgets.git')
  })

  it('服务器未配置 SSH 时该选项不可用', async () => {
    const { user } = setup(
      <CloneDropdown owner="alice" name="widgets" repoData={null} />,
    )
    await user.click(screen.getByRole('button', { name: /克隆|Clone/ }))
    const sshTab = screen.getByRole('tab', { name: 'SSH' }) as HTMLButtonElement
    expect(sshTab.disabled).toBe(true)
  })

  it('服务器返回 SSH clone URL 时可以切换过去', async () => {
    const { user } = setup(
      <CloneDropdown
        owner="alice"
        name="widgets"
        repoData={{
          clone_url_https: '/alice/widgets.git',
          clone_url_ssh: 'ssh://git@gita.example.com:2222/alice/widgets.git',
        } as never}
      />,
    )
    await user.click(screen.getByRole('button', { name: /克隆|Clone/ }))
    await user.click(screen.getByRole('tab', { name: 'SSH' }))
    const input = screen.getByRole('textbox') as HTMLInputElement
    expect(input.value).toBe('ssh://git@gita.example.com:2222/alice/widgets.git')
  })

  it('Escape 关闭面板', async () => {
    const { user } = setup(
      <CloneDropdown owner="alice" name="widgets" repoData={null} />,
    )
    const trigger = screen.getByRole('button', { name: /克隆|Clone/ })
    await user.click(trigger)
    expect(document.querySelector('.clone-dropdown-panel')).not.toBeNull()
    await user.keyboard('{Escape}')
    expect(document.querySelector('.clone-dropdown-panel')).toBeNull()
  })
})

/// Render inside the i18n provider and wire user-event.
function setup(ui: React.ReactNode) {
  const user = userEvent.setup()
  render(<I18nProvider>{ui}</I18nProvider>)
  return { user }
}
