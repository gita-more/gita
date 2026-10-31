// @vitest-environment jsdom
// Tests for the admin VM cluster panel.
//
// The panel is a thin client over three GET endpoints plus two DELETEs.
// What can actually break:
//
// 1. the cluster-disabled case — a server without [server.vm] answers
//    404, and the panel must show a notice, not an error;
// 2. the tables must render what the server returned (name, phase,
//    status, IP);
// 3. the delete buttons must call the right endpoint.
//
// The api module is mocked, following the ReadmePanel test pattern.

import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(cleanup)

vi.mock('../lib/api', () => ({
  api: {
    listVmTemplates: vi.fn(),
    listVmNodes: vi.fn(),
    listVms: vi.fn(),
    deleteVmTemplate: vi.fn(),
    deleteVm: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { AdminVmSection } from './AdminVmSection'

const listVmTemplates = vi.mocked(api.listVmTemplates)
const listVmNodes = vi.mocked(api.listVmNodes)
const listVms = vi.mocked(api.listVms)
const deleteVmTemplate = vi.mocked(api.deleteVmTemplate)
const deleteVm = vi.mocked(api.deleteVm)

/** An ApiError-shaped failure with an HTTP status, like the api module throws. */
function apiError(status: number): Error {
  const e = new Error('request failed') as Error & { status: number }
  e.status = status
  return e
}

function makeTemplate(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    id: 7,
    name: 'ci-runner',
    version: 1,
    spec: {
      cpu_millis: 4000,
      memory_bytes: 4 * 1024 * 1024 * 1024,
      disk_bytes: 20 * 1024 * 1024 * 1024,
      image: { name: 'ubuntu-24.04' },
      backend: 'qemu',
    },
    description: '',
    owner: { owner_type: 'system' as const, owner_id: 0 },
    created_at_ms: 0,
    ...overrides,
  }
}

function renderPanel() {
  return render(
    <I18nProvider>
      <AdminVmSection />
    </I18nProvider>,
  )
}

describe('AdminVmSection', () => {
  it('服务器未启用 VM 集群（404）时显示提示而不是报错', async () => {
    listVmTemplates.mockRejectedValue(apiError(404))

    renderPanel()

    // The disabled notice appears…
    await waitFor(() => {
      expect(screen.getByTestId('vm-section-disabled')).toBeTruthy()
    })
    // …and no table does.
    expect(screen.queryByTestId('vm-templates')).toBeNull()
  })

  it('渲染模板、节点、实例三张表', async () => {
    listVmTemplates.mockResolvedValue({
      templates: [makeTemplate()],
    })
    listVmNodes.mockResolvedValue({
      nodes: [
        {
          id: 3,
          name: 'worker-1',
          spec: { cpu_millis: 8000, memory_bytes: 16 * 1024 * 1024 * 1024, disk_bytes: 100 * 1024 * 1024 * 1024 },
          status: {
            alloc_cpu_millis: 2000,
            alloc_memory_bytes: 2 * 1024 * 1024 * 1024,
            alloc_disk_bytes: 0,
            running_vms: 1,
            phase: 'online',
            last_heartbeat_ms: 0,
          },
          labels: {},
        },
      ],
    })
    listVms.mockResolvedValue({
      vms: [
        {
          id: 11,
          name: 'ci-42-1730000000',
          owner: { owner_type: 'repo', owner_id: 5 },
          node: 3,
          spec: { cpu_millis: 4000, memory_bytes: 0, disk_bytes: 0 },
          status: 'running',
          ip_address: '10.42.0.100',
          created_at_ms: 0,
          expires_at_ms: null,
          message: null,
        },
      ],
    })

    renderPanel()

    // Template row: name, image, backend.
    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })
    expect(screen.getByText('ubuntu-24.04')).toBeTruthy()
    expect(screen.getByText('qemu')).toBeTruthy()

    // Node row: name + localized phase.
    expect(screen.getByText('worker-1')).toBeTruthy()
    expect(screen.getByText('在线')).toBeTruthy()

    // Instance row: name, raw status, IP.
    expect(screen.getByText('ci-42-1730000000')).toBeTruthy()
    expect(screen.getByText('running')).toBeTruthy()
    expect(screen.getByText('10.42.0.100')).toBeTruthy()
  })

  it('空集群显示各块的空状态文案', async () => {
    listVmTemplates.mockResolvedValue({ templates: [] })
    listVmNodes.mockResolvedValue({ nodes: [] })
    listVms.mockResolvedValue({ vms: [] })

    renderPanel()

    await waitFor(() => {
      expect(screen.getByTestId('vm-section')).toBeTruthy()
    })
    // No table is rendered for empty data.
    expect(screen.queryByTestId('vm-templates')).toBeNull()
    expect(screen.queryByTestId('vm-nodes')).toBeNull()
    expect(screen.queryByTestId('vm-instances')).toBeNull()
  })

  it('删除模板调用 DELETE /api/v1/vm/templates/{id} 并刷新', async () => {
    const tpl = makeTemplate()
    listVmTemplates.mockResolvedValue({ templates: [tpl] })
    listVmNodes.mockResolvedValue({ nodes: [] })
    listVms.mockResolvedValue({ vms: [] })
    deleteVmTemplate.mockResolvedValue(undefined)

    const user = userEvent.setup()
    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '删除' }))

    await waitFor(() => {
      expect(deleteVmTemplate).toHaveBeenCalledWith(7)
    })
    // The list was refreshed after deletion.
    await waitFor(() => {
      expect(listVmTemplates).toHaveBeenCalledTimes(2)
    })
  })

  it('回收实例调用 DELETE /api/v1/vm/vms/{id}', async () => {
    listVmTemplates.mockResolvedValue({ templates: [] })
    listVmNodes.mockResolvedValue({ nodes: [] })
    listVms.mockResolvedValue({
      vms: [
        {
          id: 11,
          name: 'ci-42-1730000000',
          owner: { owner_type: 'repo', owner_id: 5 },
          node: null,
          spec: { cpu_millis: 0, memory_bytes: 0, disk_bytes: 0 },
          status: 'running',
          ip_address: null,
          created_at_ms: 0,
          expires_at_ms: null,
          message: null,
        },
      ],
    })
    deleteVm.mockResolvedValue(undefined)

    const user = userEvent.setup()
    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('ci-42-1730000000')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '回收' }))

    await waitFor(() => {
      expect(deleteVm).toHaveBeenCalledWith(11)
    })
  })

  it('删除失败把错误翻译出来而不是静默', async () => {
    listVmTemplates.mockResolvedValue({ templates: [makeTemplate()] })
    listVmNodes.mockResolvedValue({ nodes: [] })
    listVms.mockResolvedValue({ vms: [] })
    deleteVmTemplate.mockRejectedValue(new Error('internal error'))

    const user = userEvent.setup()
    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '删除' }))

    // The generic English fallback for an untranslatable error.
    await waitFor(() => {
      expect(screen.getByText(/internal error|出错了|错误/)).toBeTruthy()
    })
    // And the template stays listed.
    expect(screen.getByText('ci-runner')).toBeTruthy()
  })

  it('节点离线时显示离线阶段而不是在线', async () => {
    listVmTemplates.mockResolvedValue({ templates: [] })
    listVmNodes.mockResolvedValue({
      nodes: [
        {
          id: 3,
          name: 'worker-2',
          spec: { cpu_millis: 8000, memory_bytes: 0, disk_bytes: 0 },
          status: {
            alloc_cpu_millis: 0,
            alloc_memory_bytes: 0,
            alloc_disk_bytes: 0,
            running_vms: 0,
            phase: 'offline',
            last_heartbeat_ms: 0,
          },
          labels: {},
        },
      ],
    })
    listVms.mockResolvedValue({ vms: [] })

    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('离线')).toBeTruthy()
    })
  })
})
