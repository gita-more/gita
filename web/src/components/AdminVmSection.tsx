// Administrator-only VM cluster panel.
//
// Shown in Settings when the signed-in user is an admin. Three blocks:
// templates (what CI `vm:` blocks reference), nodes (the hypervisors),
// and live instances (what is running right now).
//
// The whole panel degrades gracefully: when the server was started
// without `[server.vm] enabled`, every VM endpoint answers 404 and the
// panel shows a single "not enabled" notice instead of an error — a
// deployment that never turned the cluster on should not look broken.

import { useCallback, useEffect, useState } from 'react'
import { useI18n } from '../i18n'
import {
  api,
  type VmTemplateInfo,
  type VmNodeInfo,
  type VmInstanceInfo,
  type VmEnrollmentTokenInfo,
} from '../lib/api'
import { useErrorTranslator } from '../i18n'

/** True when the failure is "the VM cluster is off on this server" (404). */
function isClusterDisabled(err: unknown): boolean {
  return err instanceof Error && 'status' in err && (err as { status?: number }).status === 404
}

/** Human size for byte counts, e.g. 4 GiB → "4.0G". */
function fmtBytes(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) {
    const g = bytes / (1024 * 1024 * 1024)
    return `${g % 1 === 0 ? g : g.toFixed(1)}G`
  }
  if (bytes >= 1024 * 1024) {
    const m = bytes / (1024 * 1024)
    return `${m % 1 === 0 ? m : m.toFixed(1)}M`
  }
  return String(bytes)
}

export function AdminVmSection() {
  const { t } = useI18n()
  const translateError = useErrorTranslator()

  const [disabled, setDisabled] = useState(false)
  const [templates, setTemplates] = useState<VmTemplateInfo[]>([])
  const [nodes, setNodes] = useState<VmNodeInfo[]>([])
  const [vms, setVms] = useState<VmInstanceInfo[]>([])
  const [joinTokens, setJoinTokens] = useState<VmEnrollmentTokenInfo[]>([])
  const [showAddNode, setShowAddNode] = useState(false)
  const [nodeName, setNodeName] = useState('')
  const [createdJoinToken, setCreatedJoinToken] = useState<string | null>(null)
  const [createdNodeName, setCreatedNodeName] = useState('')
  const [nodeBusy, setNodeBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')

  const refresh = useCallback(async () => {
    setError('')
    try {
      const [t, n, v] = await Promise.all([
        api.listVmTemplates(),
        api.listVmNodes(),
        api.listVms(),
      ])
      setTemplates(t.templates)
      setNodes(n.nodes)
      setVms(v.vms)
      setDisabled(false)
      // Join tokens are a convenience: a failure to list them must not
      // make an otherwise healthy cluster look broken.
      try {
        const j = await api.listVmEnrollmentTokens()
        setJoinTokens(j.tokens)
      } catch {
        setJoinTokens([])
      }
    } catch (err) {
      if (isClusterDisabled(err)) {
        setDisabled(true)
      } else {
        setError(translateError(err))
      }
    }
  }, [translateError])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function removeTemplate(tpl: VmTemplateInfo) {
    setError('')
    setNotice('')
    try {
      await api.deleteVmTemplate(tpl.id)
      setNotice(t('vm.templateDeleted', { name: tpl.name }))
      await refresh()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function removeVm(vm: VmInstanceInfo) {
    setError('')
    setNotice('')
    try {
      await api.deleteVm(vm.id)
      setNotice(t('vm.vmDeleted', { name: vm.name }))
      await refresh()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function createJoinToken() {
    setError('')
    setNotice('')
    setNodeBusy(true)
    setCreatedJoinToken(null)
    try {
      const res = await api.createVmEnrollmentToken('node', nodeName.trim())
      setCreatedJoinToken(res.token)
      setCreatedNodeName(res.enrollment.node_name ?? '')
      setNodeName('')
      await refresh()
    } catch (err) {
      setError(translateError(err))
    } finally {
      setNodeBusy(false)
    }
  }

  async function revokeJoinToken(token: VmEnrollmentTokenInfo) {
    setError('')
    setNotice('')
    try {
      await api.deleteVmEnrollmentToken(token.id)
      setNotice(t('vm.joinTokenRevoked'))
      await refresh()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function removeNode(node: VmNodeInfo) {
    if (!window.confirm(t('vm.removeNodeConfirm', { name: node.name }))) return
    setError('')
    setNotice('')
    try {
      await api.deleteVmNode(node.id)
      setNotice(t('vm.nodeRemoved', { name: node.name }))
      await refresh()
    } catch (err) {
      setError(translateError(err))
    }
  }

  const nodeNameArg =
    createdNodeName === '' ? '' : ` --node-name '${createdNodeName}'`
  const joinCommand =
    createdJoinToken === null
      ? ''
      : `gita-vm-node --control-url ${window.location.origin} --join-token ${createdJoinToken}${nodeNameArg}`

  async function copyJoinCommand() {
    try {
      await navigator.clipboard.writeText(joinCommand)
      setNotice(t('vm.copied'))
    } catch {
      setNotice(t('vm.copyFailed'))
    }
  }

  if (disabled) {
    return (
      <div className="settings-section" id="vm" data-testid="vm-section-disabled">
        <h3>{t('vm.title')}</h3>
        <p>{t('vm.disabled')}</p>
      </div>
    )
  }

  return (
    <div className="settings-section" id="vm" data-testid="vm-section">
      <h3>{t('vm.title')}</h3>
      {error && <div className="alert alert-error">{error}</div>}
      {notice && <div className="alert alert-success">{notice}</div>}

      {/* ----- Templates ----- */}
      <h4>{t('vm.templatesTitle')}</h4>
      {templates.length === 0 ? (
        <p>{t('vm.noTemplates')}</p>
      ) : (
        <table className="table" data-testid="vm-templates">
          <thead>
            <tr>
              <th>{t('vm.colName')}</th>
              <th>{t('vm.colImage')}</th>
              <th>{t('vm.colResources')}</th>
              <th>{t('vm.colBackend')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {templates.map((tpl) => (
              <tr key={tpl.id}>
                <td>{tpl.name}</td>
                <td>{tpl.spec.image?.name ?? '—'}</td>
                <td>
                  {tpl.spec.cpu_millis / 1000} CPU · {fmtBytes(tpl.spec.memory_bytes)} ·{' '}
                  {fmtBytes(tpl.spec.disk_bytes)}
                </td>
                <td>{tpl.spec.backend}</td>
                <td>
                  <button
                    className="btn btn-ghost btn-sm"
                    onClick={() => void removeTemplate(tpl)}
                  >
                    {t('vm.delete')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {/* ----- Nodes ----- */}
      <div className="settings-tokens-header">
        <h4>{t('vm.nodesTitle')}</h4>
        <button
          className="btn btn-ghost btn-sm"
          onClick={() => {
            setShowAddNode((v) => !v)
            setCreatedJoinToken(null)
            setCreatedNodeName('')
          }}
        >
          {showAddNode ? t('common.cancel') : t('vm.addNode')}
        </button>
      </div>

      {showAddNode && (
        <div className="settings-section" data-testid="vm-add-node">
          <p>{t('vm.addNodeDescription')}</p>
          <label className="field-label" htmlFor="vm-node-name">
            {t('vm.nodeNameOptional')}
          </label>
          <input
            id="vm-node-name"
            className="input"
            value={nodeName}
            onChange={(e) => setNodeName(e.target.value)}
            placeholder={t('vm.nodeNamePlaceholder')}
          />
          <button
            className="btn btn-primary"
            disabled={nodeBusy}
            onClick={() => void createJoinToken()}
          >
            {nodeBusy ? t('common.loading') : t('vm.generateJoinToken')}
          </button>

          {createdJoinToken && (
            <div className="alert alert-success">
              <div>{t('vm.joinTokenCreated')}</div>
              <div>{t('vm.joinTokenOnce')}</div>
              <pre>
                <code data-testid="vm-join-command">{joinCommand}</code>
              </pre>
              <button className="btn btn-ghost btn-sm" onClick={() => void copyJoinCommand()}>
                {t('vm.copyCommand')}
              </button>
            </div>
          )}
        </div>
      )}

      {nodes.length === 0 ? (
        <p>{t('vm.noNodes')}</p>
      ) : (
        <table className="table" data-testid="vm-nodes">
          <thead>
            <tr>
              <th>{t('vm.colName')}</th>
              <th>{t('vm.colPhase')}</th>
              <th>{t('vm.colResources')}</th>
              <th>{t('vm.colRunningVms')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {nodes.map((n) => (
              <tr key={n.id}>
                <td>{n.name}</td>
                <td>
                  <span className={`vm-phase vm-phase-${n.status.phase}`}>
                    {t(`vm.phase.${n.status.phase}` as Parameters<typeof t>[0])}
                  </span>
                </td>
                <td>
                  {n.status.alloc_cpu_millis}/{n.spec.cpu_millis}m ·{' '}
                  {fmtBytes(n.status.alloc_memory_bytes)}/{fmtBytes(n.spec.memory_bytes)}
                </td>
                <td>{n.status.running_vms}</td>
                <td>
                  <button
                    className="btn btn-ghost btn-sm"
                    onClick={() => void removeNode(n)}
                  >
                    {t('vm.removeNode')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {joinTokens.some((tok) => tok.used_at_ms === null) && (
        <>
          <h4>{t('vm.pendingTokensTitle')}</h4>
          <table className="table" data-testid="vm-join-tokens">
            <thead>
              <tr>
                <th>{t('vm.colTokenName')}</th>
                <th>{t('vm.colNodeName')}</th>
                <th>{t('vm.colExpires')}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {joinTokens
                .filter((tok) => tok.used_at_ms === null)
                .map((tok) => (
                  <tr key={tok.id}>
                    <td>{tok.name || '—'}</td>
                    <td>{tok.node_name || '—'}</td>
                    <td>{new Date(tok.expires_at_ms).toLocaleString()}</td>
                    <td>
                      <button
                        className="btn btn-ghost btn-sm"
                        onClick={() => void revokeJoinToken(tok)}
                      >
                        {t('vm.revoke')}
                      </button>
                    </td>
                  </tr>
                ))}
            </tbody>
          </table>
        </>
      )}

      {/* ----- Live instances ----- */}
      <h4>{t('vm.instancesTitle')}</h4>
      {vms.length === 0 ? (
        <p>{t('vm.noInstances')}</p>
      ) : (
        <table className="table" data-testid="vm-instances">
          <thead>
            <tr>
              <th>{t('vm.colName')}</th>
              <th>{t('vm.colStatus')}</th>
              <th>{t('vm.colIp')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {vms.map((vm) => (
              <tr key={vm.id}>
                <td>{vm.name}</td>
                <td>
                  <span className={`vm-status vm-status-${vm.status}`}>{vm.status}</span>
                </td>
                <td>{vm.ip_address ?? '—'}</td>
                <td>
                  <button className="btn btn-ghost btn-sm" onClick={() => void removeVm(vm)}>
                    {t('vm.reclaim')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}
