import { useEffect, useState } from 'react'
import { useI18n } from '../i18n'
import { api, type Pipeline, type PipelineRun, type Job, type JobLogLine, type RunVmInfo } from '../lib/api'
import { PageHeader, EmptyState, StatusLabel } from './Layout'
import { RocketIcon } from './Icons'

export function PipelinesPanel({
  owner,
  name,
  pipelines,
  onChanged: _onChanged,
}: {
  owner: string
  name: string
  pipelines: Pipeline[]
  onChanged: () => void
}) {
  const { t } = useI18n()
  const [runs, setRuns] = useState<PipelineRun[]>([])
  const [selectedRunId, setSelectedRunId] = useState<number | null>(null)
  // A repository can carry `.gita-ci.yml` and any number of
  // `.github/workflows/*.yml`; the selected one decides whose runs are
  // listed. Default to the first, and fall back when the list changes.
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [triggering, setTriggering] = useState(false)
  const [triggerError, setTriggerError] = useState('')

  const activePath =
    selectedPath && pipelines.some((p) => p.path === selectedPath)
      ? selectedPath
      : pipelines[0]?.path ?? null

  const refreshRuns = () => {
    if (!activePath) {
      setRuns([])
      return
    }
    api
      .listRuns(owner, name, activePath)
      .then((r) => setRuns(r.runs))
      .catch(() => setRuns([]))
  }

  useEffect(() => {
    refreshRuns()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [owner, name, activePath, pipelines])

  async function handleTrigger() {
    if (!activePath || triggering) return
    setTriggering(true)
    setTriggerError('')
    try {
      await api.triggerRun(owner, name, activePath)
      refreshRuns()
    } catch (e) {
      setTriggerError(String(e))
    } finally {
      setTriggering(false)
    }
  }

  return (
    <>
      <PageHeader
        icon={<RocketIcon size={18} />}
        title={t('repo.pipelinesTitle')}
        subtitle={t('repo.pipelinesSubtitle', { n: runs.length })}
        actions={
          pipelines.length > 0 ? (
            <button
              className="btn btn-primary"
              onClick={() => void handleTrigger()}
              disabled={triggering}
            >
              {triggering
                ? t('repo.triggerRunning')
                : t('repo.runPipeline')}
            </button>
          ) : undefined
        }
      />

      {triggerError && (
        <div className="alert alert-error">{triggerError}</div>
      )}

      {pipelines.length > 1 && (
        <div className="pipeline-picker" data-testid="pipeline-picker">
          {pipelines.map((p) => (
            <button
              key={p.path}
              className={`btn btn-ghost${p.path === activePath ? ' is-active' : ''}`}
              onClick={() => {
                setSelectedPath(p.path)
                setSelectedRunId(null)
              }}
            >
              {p.path}
            </button>
          ))}
        </div>
      )}

      {pipelines.length === 0 ? (
        <EmptyState
          icon={<RocketIcon size={22} />}
          title={t('repo.emptyPipelinesTitle')}
          description={t('repo.emptyPipelinesDescription')}
        />
      ) : selectedRunId ? (
        <RunDetail
          owner={owner}
          name={name}
          runId={selectedRunId}
          onBack={() => setSelectedRunId(null)}
        />
      ) : runs.length === 0 ? (
        <EmptyState
          icon={<RocketIcon size={22} />}
          title={t('repo.emptyRunsTitle')}
          description={t('repo.emptyRunsDescription')}
        />
      ) : (
        <div className="row-list">
          {runs.map((run) => (
            <button
              key={run.id}
              className="row-item"
              onClick={() => setSelectedRunId(run.id)}
            >
              <span className={`row-icon status-${run.status}`}>
                <RocketIcon size={16} />
              </span>
              <div className="row-main">
                <div className="row-title">
                  #{run.id} {run.head_sha.slice(0, 10)}
                </div>
                <div className="row-meta">
                  <span>{run.ref}</span>
                  <span>{t('repo.runQueuedAt', { ts: new Date(run.queued_at).toLocaleString() })}</span>
                </div>
              </div>
              <StatusLabel status={run.status} />
            </button>
          ))}
        </div>
      )}
    </>
  )
}

function RunDetail({
  owner,
  name,
  runId,
  onBack,
}: {
  owner: string
  name: string
  runId: number
  onBack: () => void
}) {
  const { t } = useI18n()
  const [run, setRun] = useState<PipelineRun | null>(null)
  const [jobs, setJobs] = useState<Job[]>([])
  const [vms, setVms] = useState<RunVmInfo[]>([])
  const [logs, setLogs] = useState<Record<number, JobLogLine[]>>({})

  useEffect(() => {
    let cancelled = false
    api.getRun(owner, name, runId).then((r) => {
      if (cancelled) return
      setRun(r.run)
      setJobs(r.jobs)
      setVms(r.vms ?? [])
      const initial: Record<number, JobLogLine[]> = {}
      r.jobs.forEach((j) => {
        initial[j.id] = []
      })
      setLogs(initial)
    })
    return () => {
      cancelled = true
    }
  }, [owner, name, runId])

  // Subscribe to live log + status events via SSE. One connection per
  // job streams its own logs and emits a status event when it enters
  // a terminal state. The backend auto-closes each connection when
  // the job is done.
  useEffect(() => {
    if (jobs.length === 0) return
    const jobConns = jobs.map((j) =>
      api.streamJobLogs(owner, name, runId, j.id, {
        onLog: (line) => {
          setLogs((prev) => ({
            ...prev,
            [j.id]: [...(prev[j.id] ?? []), line],
          }))
        },
        onStatus: (job) => {
          setJobs((prev) =>
            prev.map((pj) => (pj.id === job.id ? job : pj)),
          )
        },
        onError: (e) => {
          console.warn(`log stream for job ${j.id} errored`, e)
        },
      }),
    )
    return () => jobConns.forEach((c) => c.close())
  }, [owner, name, runId, jobs.length])

  if (!run) return null
  return (
    <>
      <div className="pr-detail-header">
        <div className="pr-detail-title">
          <span className="pr-number">#{run.id}</span>
          {run.head_sha.slice(0, 10)}
          <StatusLabel status={run.status} />
        </div>
        <div className="page-actions">
          <button className="btn btn-ghost" onClick={onBack}>
            {t('repo.backToList')}
          </button>
        </div>
      </div>
      {/* VM instances backing this run's vm: jobs. Absent for local
          runs, so the block only renders when the server reports any. */}
      {vms.length > 0 && (
        <div className="run-vms" data-testid="run-vms">
          <h4>{t('repo.runVmsTitle')}</h4>
          <table className="table">
            <tbody>
              {vms.map((vm) => (
                <tr key={vm.id}>
                  <td>{vm.name}</td>
                  <td>
                    <span className={`vm-status vm-status-${vm.status}`}>{vm.status}</span>
                  </td>
                  <td>{vm.ip_address ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="commit-list">
        {jobs.map((job) => (
          <div key={job.id} className="commit-item">
            <span className="commit-avatar">{job.name.slice(0, 1).toUpperCase()}</span>
            <div className="commit-body">
              <div className="commit-message">
                {job.name} <StatusLabel status={job.status} />
              </div>
              <pre className="code-block" data-job-id={job.id}>
                {(logs[job.id] ?? [])
                  .map((l) => `[${l.stream}] ${l.line}`)
                  .join('\n') || t('repo.noLogsYet')}
              </pre>
            </div>
          </div>
        ))}
      </div>
    </>
  )
}