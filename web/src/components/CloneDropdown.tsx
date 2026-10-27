import { useEffect, useRef, useState } from 'react'
import { useI18n } from '../i18n'
import type { Repository } from '../lib/api'
import { buildCloneUrl } from './EmptyRepoWelcome'
import { CheckIcon, ChevronDownIcon, CodeIcon, CopyIcon, TerminalIcon } from './Icons'

type Proto = 'https' | 'ssh'

/**
 * GitHub-style "Code" dropdown for the repo code page: protocol switch
 * (HTTPS / SSH), readonly clone URL input and a copy-to-clipboard button.
 */
export function CloneDropdown({
  owner,
  name,
  repoKind,
  groupName,
  repoData,
}: {
  owner: string
  name: string
  repoKind?: 'personal' | 'group'
  groupName?: string
  repoData: Repository | null
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [proto, setProto] = useState<Proto>('https')
  const [copied, setCopied] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  // Close on Escape / click outside, matching the usual popover contract.
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    const onClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('keydown', onKey)
    document.addEventListener('mousedown', onClick)
    return () => {
      document.removeEventListener('keydown', onKey)
      document.removeEventListener('mousedown', onClick)
    }
  }, [open])

  const fallbackHttpsUrl = buildCloneUrl(
    owner,
    name,
    repoKind ?? 'personal',
    groupName,
  )
  // The API returns a path-relative HTTPS URL (/owner/repo.git); make it
  // absolute for display and copy (same as EmptyRepoWelcome).
  const absolute = (url: string) =>
    url.startsWith('/') ? `${window.location.origin}${url}` : url
  const httpsUrl = repoData?.clone_url_https
    ? absolute(repoData.clone_url_https)
    : fallbackHttpsUrl
  const sshUrl = repoData?.clone_url_ssh ?? null
  const cloneUrl = proto === 'https' ? httpsUrl : (sshUrl ?? '')

  async function handleCopy() {
    if (!cloneUrl || !navigator?.clipboard) return
    try {
      await navigator.clipboard.writeText(cloneUrl)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // Clipboard may be blocked; the input stays selectable for manual copy.
    }
  }

  return (
    <div className="clone-dropdown" ref={rootRef}>
      <button
        type="button"
        className="btn btn-primary btn-sm clone-dropdown-trigger"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="true"
      >
        <CodeIcon size={14} />
        {t('repo.clone.title')}
        <ChevronDownIcon size={12} />
      </button>
      {open && (
        <div className="clone-dropdown-panel" role="region" aria-label={t('repo.clone.title')}>
          <div className="clone-dropdown-header">
            <TerminalIcon size={14} />
            <span className="clone-dropdown-title">{t('repo.clone.title')}</span>
          </div>
          <div className="clone-dropdown-proto" role="tablist" aria-label={t('repo.clone.protocol')}>
            <button
              type="button"
              role="tab"
              aria-selected={proto === 'https'}
              className={`proto-toggle-btn${proto === 'https' ? ' active' : ''}`}
              onClick={() => setProto('https')}
            >
              HTTPS
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={proto === 'ssh'}
              className={`proto-toggle-btn${proto === 'ssh' ? ' active' : ''}`}
              onClick={() => setProto('ssh')}
              disabled={!sshUrl}
              title={sshUrl ? undefined : t('repo.clone.sshPending')}
            >
              SSH
            </button>
          </div>
          <div className="clone-url">
            <input
              type="text"
              readOnly
              value={cloneUrl}
              aria-label={t('repo.clone.urlLabel')}
              className="clone-url-input"
              onClick={(e) => (e.target as HTMLInputElement).select()}
            />
            <button
              type="button"
              className="btn btn-ghost btn-sm clone-dropdown-copy"
              onClick={() => void handleCopy()}
              disabled={!cloneUrl}
              aria-label={t('repo.clone.copyTooltip')}
              title={t('repo.clone.copyTooltip')}
            >
              {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
            </button>
          </div>
          {proto === 'ssh' && !sshUrl && (
            <p className="clone-dropdown-note">{t('repo.clone.sshPending')}</p>
          )}
          <p className="clone-dropdown-footer">{t('repo.clone.footer')}</p>
        </div>
      )}
    </div>
  )
}
