// Tests for LoginPage — the sign-in / sign-up card.
//
// The page delegates the actual API calls to the useAuth hook, which in turn
// calls api.login / api.register. The api module is mocked so the tests
// control what the server answers.
//
// What can actually break:
//  1. the credentials must reach the API unchanged;
//  2. a rejected login must show an error, not a blank card;
//  3. the submit button must be disabled while the request is in flight;
//  4. switching to the register mode must add the email field and call the
//     register endpoint instead of the login one.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    login: vi.fn(),
    register: vi.fn(),
    updateLocale: vi.fn(),
  },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { LoginPage } from './LoginPage'

const login = vi.mocked(api.login)
const register = vi.mocked(api.register)

function setup(onDone = vi.fn()) {
  const user = userEvent.setup()
  render(
    <I18nProvider>
      <LoginPage onDone={onDone} />
    </I18nProvider>,
  )
  return { user, onDone }
}

function makeUser() {
  return {
    id: 1,
    username: 'alice',
    email: 'alice@example.com',
    locale: 'zh',
    is_admin: false,
  }
}

describe('LoginPage', () => {
  it('渲染登录表单', () => {
    setup()

    expect(screen.getByLabelText('用户名')).toBeTruthy()
    expect(screen.getByLabelText('密码')).toBeTruthy()
    // The email field only exists in register mode.
    expect(screen.queryByLabelText('邮箱')).toBeNull()
    expect(screen.getByRole('button', { name: '登录' })).toBeTruthy()
  })

  it('提交用户名和密码到登录接口，成功后回调 onDone', async () => {
    login.mockResolvedValue({ token: 'tok', user: makeUser() })
    const { user, onDone } = setup()

    await user.type(screen.getByLabelText('用户名'), 'alice')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '登录' }))

    await waitFor(() => {
      expect(login).toHaveBeenCalledWith('alice', 'secret123')
    })
    expect(onDone).toHaveBeenCalled()
    expect(register).not.toHaveBeenCalled()
  })

  it('登录失败时显示错误信息且不回调 onDone', async () => {
    login.mockRejectedValue(new Error('invalid credentials'))
    const { user, onDone } = setup()

    await user.type(screen.getByLabelText('用户名'), 'alice')
    await user.type(screen.getByLabelText('密码'), 'wrongpass')
    await user.click(screen.getByRole('button', { name: '登录' }))

    await waitFor(() => {
      expect(screen.getByText('用户名或密码错误')).toBeTruthy()
    })
    expect(onDone).not.toHaveBeenCalled()
  })

  it('请求进行中禁用提交按钮并显示处理中', async () => {
    let resolveLogin: ((v: { token: string; user: ReturnType<typeof makeUser> }) => void) | undefined
    login.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveLogin = resolve
        }),
    )
    const { user } = setup()

    await user.type(screen.getByLabelText('用户名'), 'alice')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '登录' }))

    const busy = await screen.findByRole('button', { name: '处理中…' })
    expect((busy as HTMLButtonElement).disabled).toBe(true)

    resolveLogin?.({ token: 'tok', user: makeUser() })
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '登录' })).toBeTruthy()
    })
  })

  it('切到注册模式后多出邮箱字段并调用注册接口', async () => {
    register.mockResolvedValue({ token: 'tok', user: makeUser() })
    const { user, onDone } = setup()

    await user.click(screen.getByRole('button', { name: '立即注册' }))

    expect(screen.getByLabelText('邮箱')).toBeTruthy()
    expect(screen.getByRole('button', { name: '注册' })).toBeTruthy()

    await user.type(screen.getByLabelText('用户名'), 'alice')
    await user.type(screen.getByLabelText('邮箱'), 'alice@example.com')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '注册' }))

    await waitFor(() => {
      expect(register).toHaveBeenCalledWith('alice', 'alice@example.com', 'secret123')
    })
    expect(login).not.toHaveBeenCalled()
    expect(onDone).toHaveBeenCalled()
  })

  it('注册失败时显示错误', async () => {
    register.mockRejectedValue(new Error('email already registered'))
    const { user, onDone } = setup()

    await user.click(screen.getByRole('button', { name: '立即注册' }))
    await user.type(screen.getByLabelText('用户名'), 'alice')
    await user.type(screen.getByLabelText('邮箱'), 'alice@example.com')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '注册' }))

    await waitFor(() => {
      expect(screen.getByText('邮箱已被注册')).toBeTruthy()
    })
    expect(onDone).not.toHaveBeenCalled()
  })

  it('可以切回登录模式', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: '立即注册' }))
    expect(screen.getByLabelText('邮箱')).toBeTruthy()

    await user.click(screen.getByRole('button', { name: '去登录' }))
    expect(screen.queryByLabelText('邮箱')).toBeNull()
    expect(screen.getByRole('button', { name: '登录' })).toBeTruthy()
  })
})
