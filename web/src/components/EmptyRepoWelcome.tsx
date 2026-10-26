import { useState } from 'react'
import { useI18n } from '../i18n'
import { Alert } from './Layout'

export interface EmptyRepoWelcomeProps {
  owner: string
  name: string
  repoKind: 'personal' | 'group'
  groupName?: string
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
  // The server mounts the git smart-HTTP endpoints under `/git/`. A clone
  // URL without that prefix hits the SPA fallback and fails.
  //
  // A group's repositories live under the group's *full path* on disk (the
  // namespace segment is the group's full path, e.g. "groupa/subgroupa"),
  // so the URL is `/git/<full-path>/<repo>.git`.
  if (repoKind === 'group' && groupName) {
    return `${base}/git/${groupName}/${name}.git`
  }
  return `${base}/git/${owner}/${name}.git`
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
}: EmptyRepoWelcomeProps) {
  const { t } = useI18n()
  const [proto, setProto] = useState<Proto>('https')
  const [copied, setCopied] = useState(false)
  const cloneUrl = buildCloneUrl(owner, name, repoKind, groupName)

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
          >
            {t('repo.empty.protoSsh')}
          </button>
        </div>
        <div className="clone-url">
          <input
            type="text"
            readOnly
            value={proto === 'https' ? cloneUrl : ''}
            aria-label="clone url"
            className="clone-url-input"
            onClick={(e) => (e.target as HTMLInputElement).select()}
          />
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => void handleCopy()}
            disabled={proto === 'ssh' || !cloneUrl}
          >
            {copied ? t('repo.empty.copied') : t('repo.empty.copy')}
          </button>
        </div>
        {proto === 'ssh' && (
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