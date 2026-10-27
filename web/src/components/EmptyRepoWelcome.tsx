import { useState } from 'react'
import { useI18n } from '../i18n'
import { Alert } from './Layout'

export interface EmptyRepoWelcomeProps {
  owner: string
  name: string
  repoKind: 'personal' | 'group'
  groupName?: string
  /** HTTPS clone URL from the API */
  cloneUrlHttps?: string
  /** SSH clone URL from the API (null if SSH not enabled) */
  cloneUrlSsh?: string | null
}

type Proto = 'https' | 'ssh'

function buildCloneUrl(
  owner: string,
  name: string,
  repoKind: 'personal' | 'group',
  groupName?: string,
): string {
  if (typeof window === 'undefined') return ''
  const proto = window.location.protocol
  const host = window.location.host
  const base = `${proto}//${host}`
  // Git smart-HTTP is mounted at the root as `/{owner}/{repo}.git`, so the
  // clone URL carries no prefix — matching GitLab/GitHub.
  //
  // A group's repositories live under the group's *full path* on disk (the
  // namespace segment is the group's full path, e.g. "groupa/subgroupa"),
  // so the URL is `/<full-path>/<repo>.git`.
  if (repoKind === 'group' && groupName) {
    return `${base}/${groupName}/${name}.git`
  }
  return `${base}/${owner}/${name}.git`
}

function buildCreateCommand(name: string): string {
  return [
    'echo "# ' + name + '" >> README.md',
    'git init',
    'git add README.md',
    'git commit -m "first commit"',
    'git branch -M main',
    'git remote add origin <REMOTE_URL>',
    'git push -u origin main',
  ].join('\n')
}

function buildPushCommand(): string {
  return [
    'git remote add origin <REMOTE_URL>',
    'git branch -M main',
    'git push -u origin main',
  ].join('\n')
}

export function EmptyRepoWelcome({
  owner,
  name,
  repoKind,
  groupName,
  cloneUrlHttps,
  cloneUrlSsh,
}: EmptyRepoWelcomeProps) {
  const { t } = useI18n()
  const [proto, setProto] = useState<Proto>('https')
  const [copied, setCopied] = useState(false)
  // Use API-provided URLs, fall back to client-side generation for backwards compatibility
  const fallbackHttpsUrl = buildCloneUrl(owner, name, repoKind, groupName)
  // The API returns a path-relative HTTPS URL (/owner/repo.git); make it
  // absolute for display and copy.
  const absolute = (url: string) =>
    url.startsWith('/') ? `${window.location.origin}${url}` : url
  const httpsUrl = cloneUrlHttps ? absolute(cloneUrlHttps) : fallbackHttpsUrl
  const sshUrl = cloneUrlSsh
  const cloneUrl = proto === 'https' ? httpsUrl : (sshUrl || '')

  // Render the clone/push snippets with a real placeholder so users can
  // copy and replace; we'll substitute the real URL on copy via the prompt.
  const createSnippet = buildCreateCommand(name).replace(/<REMOTE_URL>/g, cloneUrl)
  const pushSnippet = buildPushCommand().replace(/<REMOTE_URL>/g, cloneUrl)

  async function handleCopy() {
    if (!cloneUrl || !navigator?.clipboard) return
    try {
      await navigator.clipboard.writeText(cloneUrl)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // Clipboard may be blocked; leave URL visible so the user can copy it
      // manually from the input.
    }
  }

  return (
    <div className="quick-setup">
      <h2 className="quick-setup-title">{t('repo.empty.title')}</h2>
      <p className="quick-setup-desc">{t('repo.empty.desc')}</p>

      {/* Clone + protocol switch */}
      <div className="quick-setup-row">
        <div className="proto-toggle" role="tablist" aria-label="protocol">
          <button
            type="button"
            role="tab"
            aria-selected={proto === 'https'}
            className={`proto-toggle-btn${proto === 'https' ? ' active' : ''}`}
            onClick={() => setProto('https')}
          >
            {t('repo.empty.protoHttps')}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={proto === 'ssh'}
            className={`proto-toggle-btn${proto === 'ssh' ? ' active' : ''}`}
            onClick={() => setProto('ssh')}
            disabled={!sshUrl}
          >
            {t('repo.empty.protoSsh')}
          </button>
        </div>
        <div className="clone-url">
          <input
            type="text"
            readOnly
            value={cloneUrl}
            aria-label="clone url"
            className="clone-url-input"
            onClick={(e) => (e.target as HTMLInputElement).select()}
          />
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => void handleCopy()}
            disabled={!cloneUrl}
          >
            {copied ? t('repo.empty.copied') : t('repo.empty.copy')}
          </button>
        </div>
        {proto === 'ssh' && !sshUrl && (
          <Alert type="info">{t('repo.empty.sshPending')}</Alert>
        )}
      </div>

      {/* Create a new repository command block */}
      <div className="cmd-block">
        <h3 className="cmd-block-title">{t('repo.empty.cliTitleA')}</h3>
        <pre className="cmd-block-body">
          <code>{createSnippet}</code>
        </pre>
      </div>

      {/* Push an existing repository command block */}
      <div className="cmd-block">
        <h3 className="cmd-block-title">{t('repo.empty.cliTitleB')}</h3>
        <pre className="cmd-block-body">
          <code>{pushSnippet}</code>
        </pre>
      </div>

      <p className="quick-setup-protip">{t('repo.empty.protip')}</p>
    </div>
  )
}