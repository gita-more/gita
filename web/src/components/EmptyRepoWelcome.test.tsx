// @vitest-environment jsdom
// Tests for EmptyRepoWelcome's clone-URL block.
//
// The SSH tab is the interesting part: when the server has no SSH
// listener there is no SSH clone URL, and the tab must still be
// switchable — otherwise the "SSH is not configured" notice below it
// can never be displayed. The button used to be disabled in that case,
// which made the notice dead UI.

import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'

afterEach(cleanup)

import { I18nProvider } from '../i18n'
import { EmptyRepoWelcome } from './EmptyRepoWelcome'

function renderWelcome(props: Partial<React.ComponentProps<typeof EmptyRepoWelcome>> = {}) {
  return render(
    <I18nProvider>
      <EmptyRepoWelcome
        owner="alice"
        name="widgets"
        repoKind="personal"
        cloneUrlHttps="/alice/widgets.git"
        cloneUrlSsh={null}
        {...props}
      />
    </I18nProvider>,
  )
}

describe('EmptyRepoWelcome 的 SSH tab', () => {
  it('服务器没开 SSH 时，SSH tab 仍可切换（否则提示永远看不到）', async () => {
    const user = userEvent.setup()
    renderWelcome({ cloneUrlSsh: null })

    const sshTab = screen.getByRole('tab', { name: /SSH/i })
    // The tab must not be disabled: a dead button gives the user nothing.
    expect((sshTab as HTMLButtonElement).disabled).toBe(false)

    await user.click(sshTab)
    expect(sshTab.getAttribute('aria-selected')).toBe('true')
  })

  it('切到 SSH 后 URL 输入框清空、复制按钮 disabled、并显示未配置提示', async () => {
    const user = userEvent.setup()
    renderWelcome({ cloneUrlSsh: null })

    await user.click(screen.getByRole('tab', { name: /SSH/i }))

    // The URL field is empty — there is no SSH URL to show.
    const input = screen.getByLabelText('clone url') as HTMLInputElement
    expect(input.value).toBe('')

    // Nothing to copy.
    const copyBtn = screen.getByRole('button', { name: /复制|Copy/i })
    expect((copyBtn as HTMLButtonElement).disabled).toBe(true)

    // And the notice explains why. Match the specific text: the SSH tab
    // itself also contains "SSH".
    expect(screen.getByText(/SSH 暂未配置|SSH is not configured/)).toBeTruthy()
  })

  it('服务器开了 SSH 时，SSH tab 显示真实的 SSH URL', async () => {
    const user = userEvent.setup()
    renderWelcome({ cloneUrlSsh: 'git@localhost:alice/widgets.git' })

    await user.click(screen.getByRole('tab', { name: /SSH/i }))

    const input = screen.getByLabelText('clone url') as HTMLInputElement
    expect(input.value).toBe('git@localhost:alice/widgets.git')

    // With a URL present the copy button works.
    const copyBtn = screen.getByRole('button', { name: /复制|Copy/i })
    expect((copyBtn as HTMLButtonElement).disabled).toBe(false)
  })

  it('默认停在 HTTPS tab，URL 是 HTTPS 的那个', () => {
    renderWelcome()
    const input = screen.getByLabelText('clone url') as HTMLInputElement
    expect(input.value).toContain('/alice/widgets.git')
    expect(
      screen.getByRole('tab', { name: /HTTPS/i }).getAttribute('aria-selected'),
    ).toBe('true')
  })
})
