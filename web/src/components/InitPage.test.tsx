// Tests for InitPage — the first-run "create the administrator" form.
//
// The page calls api.init (through the useAuth hook) with the username,
// email and password. What can actually break:
//  1. the three fields must reach the API unchanged;
//  2. a rejected init must show an error;
//  3. the submit button must be disabled while the request is in flight;
//  4. the page must never offer a "sign in instead" switch — there is no
//    account to sign in to yet.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    init: vi.fn(),
    updateLocale: vi.fn(),
  },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { InitPage } from './InitPage'

const init = vi.mocked(api.init)

function setup(onDone = vi.fn()) {
  const user = userEvent.setup()
  render(
    <I18nProvider>
      <InitPage onDone={onDone} />
    </I18nProvider>,
  )
  return { user, onDone }
}

function makeUser() {
  return {
    id: 1,
    username: 'admin',
    email: 'admin@example.com',
    locale: 'zh',
    is_admin: true,
  }
}

describe('InitPage', () => {
  it('渲染初始化表单', () => {
    setup()

    expect(screen.getByLabelText('用户名')).toBeTruthy()
    expect(screen.getByLabelText('邮箱')).toBeTruthy()
    expect(screen.getByLabelText('密码')).toBeTruthy()
    expect(screen.getByRole('button', { name: '创建管理员' })).toBeTruthy()
    // There is no "sign in instead" option on the first-run page.
    expect(screen.queryByRole('button', { name: '去登录' })).toBeNull()
  })

  it('提交表单调用初始化接口并回调 onDone', async () => {
    init.mockResolvedValue({ token: 'tok', user: makeUser() })
    const { user, onDone } = setup()

    await user.type(screen.getByLabelText('用户名'), 'admin')
    await user.type(screen.getByLabelText('邮箱'), 'admin@example.com')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '创建管理员' }))

    await waitFor(() => {
      expect(init).toHaveBeenCalledWith('admin', 'admin@example.com', 'secret123')
    })
    expect(onDone).toHaveBeenCalled()
  })

  it('初始化失败时显示错误且不回调 onDone', async () => {
    init.mockRejectedValue(new Error('username already taken'))
    const { user, onDone } = setup()

    await user.type(screen.getByLabelText('用户名'), 'admin')
    await user.type(screen.getByLabelText('邮箱'), 'admin@example.com')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '创建管理员' }))

    await waitFor(() => {
      expect(screen.getByText('用户名已被占用')).toBeTruthy()
    })
    expect(onDone).not.toHaveBeenCalled()
  })

  it('请求进行中禁用提交按钮', async () => {
    let resolveInit: ((v: { token: string; user: ReturnType<typeof makeUser> }) => void) | undefined
    init.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveInit = resolve
        }),
    )
    const { user } = setup()

    await user.type(screen.getByLabelText('用户名'), 'admin')
    await user.type(screen.getByLabelText('邮箱'), 'admin@example.com')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '创建管理员' }))

    const busy = await screen.findByRole('button', { name: '处理中…' })
    expect((busy as HTMLButtonElement).disabled).toBe(true)

    resolveInit?.({ token: 'tok', user: makeUser() })
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '创建管理员' })).toBeTruthy()
    })
  })
})
