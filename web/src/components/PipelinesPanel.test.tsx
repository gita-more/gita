// @vitest-environment jsdom
// Tests for PipelinesPanel — the CI runs view.
//
// The panel lists the runs of the repository's first pipeline and lets the
// user trigger a new run. Clicking a run opens the run detail, which shows
// the jobs of the run and streams their logs.
//
// What can actually break:
//  1. without a pipeline the panel must show the "no pipeline" empty state;
//  2. the runs must be listed with their number, sha and status;
//  3. clicking a run must open its detail and load the jobs;
//  4. the trigger button must call the trigger endpoint and refresh;
//  5. a failed trigger must show the error.

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(cleanup)

vi.mock('../lib/api', () => ({
  api: {
    listRuns: vi.fn(),
    triggerRun: vi.fn(),
    getRun: vi.fn(),
    streamJobLogs: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { PipelinesPanel } from './PipelinesPanel'
import type { Job, Pipeline, PipelineRun } from '../lib/api'

const listRuns = vi.mocked(api.listRuns)
const triggerRun = vi.mocked(api.triggerRun)
const getRun = vi.mocked(api.getRun)
const streamJobLogs = vi.mocked(api.streamJobLogs)

function makePipeline(overrides: Partial<Pipeline> = {}): Pipeline {
  return {
    id: 1,
    repo_id: 1,
    path: '.gita-ci.yml',
    ref: 'main',
    ref_sha: 'abc123',
    content: 'stages: [build]',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

function makeRun(overrides: Partial<PipelineRun> = {}): PipelineRun {
  return {
    id: 42,
    pipeline_id: 1,
    repo_id: 1,
    ref: 'main',
    head_sha: 'abcdef0123456789',
    trigger_event_id: null,
    trigger_pusher_id: null,
    status: 'success',
    queued_at: '2026-01-01T00:00:00Z',
    started_at: '2026-01-01T00:00:01Z',
    finished_at: '2026-01-01T00:00:05Z',
    ...overrides,
  }
}

function makeJob(overrides: Partial<Job> = {}): Job {
  return {
    id: 7,
    run_id: 42,
    name: 'build',
    stage: 'build',
    needs: [],
    script: ['echo hi'],
    status: 'success',
    queued_at: '2026-01-01T00:00:00Z',
    started_at: '2026-01-01T00:00:01Z',
    finished_at: '2026-01-01T00:00:05Z',
    exit_code: 0,
    ...overrides,
  }
}

/** A no-op log stream; the panel closes it on unmount. */
function stubStream() {
  const close = vi.fn()
  streamJobLogs.mockImplementation(() => ({ close }))
  return close
}

function setup(pipelines: Pipeline[] = [makePipeline()]) {
  const user = userEvent.setup()
  render(
    <I18nProvider>
      <PipelinesPanel
        owner="alice"
        name="widgets"
        pipelines={pipelines}
        onChanged={vi.fn()}
      />
    </I18nProvider>,
  )
  return { user }
}

describe('PipelinesPanel', () => {
  it('没有流水线配置时显示空状态且不拉取运行记录', async () => {
    listRuns.mockResolvedValue({ runs: [] })

    setup([])

    await waitFor(() => {
      expect(screen.getByText('还没有流水线配置')).toBeTruthy()
    })
    expect(listRuns).not.toHaveBeenCalled()
  })

  it('列出运行记录：编号、sha 和状态', async () => {
    listRuns.mockResolvedValue({ runs: [makeRun()] })

    setup()

    await waitFor(() => {
      expect(screen.getByText('#42 abcdef0123')).toBeTruthy()
    })
    expect(screen.getByText('main')).toBeTruthy()
    // Pipeline statuses have no translation entry, so the raw status shows.
    expect(screen.getByText('success')).toBeTruthy()
    expect(screen.getByText('共 1 次运行')).toBeTruthy()
  })

  it('按第一个流水线的路径拉取运行记录', async () => {
    listRuns.mockResolvedValue({ runs: [] })

    setup([makePipeline({ path: '.gita-ci.yml' })])

    await waitFor(() => {
      expect(listRuns).toHaveBeenCalledWith('alice', 'widgets', '.gita-ci.yml')
    })
  })

  it('一个仓库有多个工作流时，可以切换并只拉取选中的那个', async () => {
    listRuns.mockResolvedValue({ runs: [] })

    // `.gita-ci.yml` and a GitHub-style workflow live side by side; the
    // panel must not silently show only the first — that is how a workflow
    // run becomes invisible in the UI.
    const { user } = setup([
      makePipeline({ path: '.gita-ci.yml' }),
      makePipeline({ path: '.github/workflows/ci.yml' }),
    ])

    await waitFor(() => {
      expect(listRuns).toHaveBeenCalledWith('alice', 'widgets', '.gita-ci.yml')
    })

    const picker = screen.getByTestId('pipeline-picker')
    await user.click(within(picker).getByText('.github/workflows/ci.yml'))

    await waitFor(() => {
      expect(listRuns).toHaveBeenCalledWith(
        'alice',
        'widgets',
        '.github/workflows/ci.yml',
      )
    })
  })

  it('只有一个流水线时不显示选择器', async () => {
    listRuns.mockResolvedValue({ runs: [] })

    setup()

    await waitFor(() => {
      expect(screen.queryByTestId('pipeline-picker')).toBeNull()
    })
  })

  it('没有运行记录时显示空状态', async () => {
    listRuns.mockResolvedValue({ runs: [] })

    setup()

    await waitFor(() => {
      expect(screen.getByText('还没有运行')).toBeTruthy()
    })
  })

  it('点击运行记录打开详情并加载 job', async () => {
    listRuns.mockResolvedValue({ runs: [makeRun()] })
    getRun.mockResolvedValue({ run: makeRun(), jobs: [makeJob()], vms: [] })
    const close = stubStream()

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('#42 abcdef0123')).toBeTruthy()
    })
    await user.click(screen.getByText('#42 abcdef0123'))

    await waitFor(() => {
      expect(getRun).toHaveBeenCalledWith('alice', 'widgets', 42)
    })
    // The job name shows in the detail view.
    expect(screen.getByText(/build/)).toBeTruthy()
    // And the live log stream was opened for the job.
    await waitFor(() => {
      expect(streamJobLogs).toHaveBeenCalledWith(
        'alice',
        'widgets',
        42,
        7,
        expect.anything(),
      )
    })
    expect(close).toBeDefined()
  })

  it('详情里有 VM 时列出虚拟机', async () => {
    listRuns.mockResolvedValue({ runs: [makeRun()] })
    getRun.mockResolvedValue({
      run: makeRun(),
      jobs: [makeJob()],
      vms: [
        { id: 3, name: 'ci-runner-1', status: 'running', node_id: 1, ip_address: '10.0.0.5', expires_at_ms: null },
      ],
    })
    stubStream()

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('#42 abcdef0123')).toBeTruthy()
    })
    await user.click(screen.getByText('#42 abcdef0123'))

    await waitFor(() => {
      expect(screen.getByTestId('run-vms')).toBeTruthy()
    })
    expect(screen.getByText('ci-runner-1')).toBeTruthy()
    expect(screen.getByText('10.0.0.5')).toBeTruthy()
  })

  it('可以从详情返回列表', async () => {
    listRuns.mockResolvedValue({ runs: [makeRun()] })
    getRun.mockResolvedValue({ run: makeRun(), jobs: [makeJob()], vms: [] })
    stubStream()

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('#42 abcdef0123')).toBeTruthy()
    })
    await user.click(screen.getByText('#42 abcdef0123'))
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '返回列表' })).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '返回列表' }))
    expect(screen.getByText('#42 abcdef0123')).toBeTruthy()
  })

  it('触发流水线调用触发接口并刷新运行列表', async () => {
    listRuns.mockResolvedValue({ runs: [] })
    triggerRun.mockResolvedValue({ run: makeRun() })

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '运行流水线' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '运行流水线' }))

    await waitFor(() => {
      expect(triggerRun).toHaveBeenCalledWith('alice', 'widgets', '.gita-ci.yml')
    })
    // The list was refreshed after the trigger.
    await waitFor(() => {
      expect(listRuns).toHaveBeenCalledTimes(2)
    })
  })

  it('触发失败时显示错误', async () => {
    listRuns.mockResolvedValue({ runs: [] })
    triggerRun.mockRejectedValue(new Error('no ci runner available'))

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '运行流水线' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '运行流水线' }))

    await waitFor(() => {
      expect(screen.getByText(/no ci runner available/)).toBeTruthy()
    })
  })
})
