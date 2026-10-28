import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n, useErrorTranslator } from '../i18n'
import { api, type Group, type Repository } from '../lib/api'
import { Alert, ConfirmDialog, PageHeader } from './Layout'
import { GitBranchIcon, GlobeIcon, PlusIcon, SettingsIcon, TagIcon } from './Icons'
import { PagesPanel } from './PagesPanel'

type Section = 'general' | 'branches' | 'tags' | 'pages'

const SECTIONS: Section[] = ['general', 'branches', 'tags', 'pages']

/** Leading icon for a settings section, mirroring GitHub's nav items. */
function SectionIcon({ section }: { section: Section }) {
  if (section === 'branches') return <GitBranchIcon size={16} />
  if (section === 'tags') return <TagIcon size={16} />
  if (section === 'pages') return <GlobeIcon size={16} />
  return <SettingsIcon size={16} />
}

/**
 * Repository settings page (`/{owner}/{repo}/settings`).
 *
 * Sections: general (name/description/visibility, danger zone), branches,
 * tags and Pages. The section is picked from `?section=` so links can deep
 * into one of them (e.g. "View all branches").
 */
export function RepoSettings({
  owner,
  name,
  section,
  onSectionChange,
  onBack,
}: {
  owner: string
  name: string
  section: string | null
  onSectionChange: (section: Section) => void
  onBack: () => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const navigate = useNavigate()
  const [repo, setRepo] = useState<Repository | null>(null)
  const [branches, setBranches] = useState<string[]>([])
  const [tags, setTags] = useState<string[]>([])
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)

  // General section form state.
  const [newName, setNewName] = useState(name)
  const [description, setDescription] = useState('')
  const [isPrivate, setIsPrivate] = useState(false)
  const [saving, setSaving] = useState(false)

  // Branch / tag creation.
  const [branchName, setBranchName] = useState('')
  const [branchRef, setBranchRef] = useState('')
  const [tagName, setTagName] = useState('')
  const [tagRef, setTagRef] = useState('')

  // Danger zone.
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)

  // Moving the repository to another namespace.
  const [groups, setGroups] = useState<Group[]>([])
  const [targetOwner, setTargetOwner] = useState('')
  const [confirmMove, setConfirmMove] = useState(false)
  const [moving, setMoving] = useState(false)

  const active: Section = SECTIONS.includes(section as Section) ? (section as Section) : 'general'

  const loadRepo = useCallback(async () => {
    try {
      const res = await api.getRepo(owner, name)
      setRepo(res)
      setNewName(res.name)
      setDescription(res.description ?? '')
      setIsPrivate(res.is_private)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, translateError])

  const loadRefs = useCallback(async () => {
    try {
      const [bs, ts] = await Promise.all([
        api.getBranches(owner, name),
        api.listTags(owner, name),
      ])
      setBranches(bs)
      setTags(ts)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, translateError])

  useEffect(() => {
    void loadRepo()
    void loadRefs()
  }, [loadRepo, loadRefs])

  // Transfer targets: the groups the current user belongs to. The backend
  // resolves `new_owner` as a top-level group name, so nested subgroups are
  // not offered here.
  useEffect(() => {
    let cancelled = false
    api
      .listGroups()
      .then((list) => {
        if (cancelled) return
        setGroups(list.filter((g) => g.parent_id === null && g.full_path !== owner))
      })
      .catch(() => {
        // Not being able to list groups only disables the transfer control.
      })
    return () => {
      cancelled = true
    }
  }, [owner])

  async function saveGeneral() {
    setError('')
    setSaved(false)
    setSaving(true)
    try {
      const patch: { description?: string; is_private?: boolean; new_name?: string } = {
        description,
        is_private: isPrivate,
      }
      if (newName.trim() && newName.trim() !== name) patch.new_name = newName.trim()
      const res = await api.updateRepo(owner, name, patch)
      setRepo(res)
      setNewName(res.name)
      setSaved(true)
      // A rename moves the repository: follow it to the new URL.
      if (res.name !== name) {
        navigate(`/${owner}/${res.name}/settings?section=${active}`, { replace: true })
      }
    } catch (err) {
      setError(translateError(err))
    } finally {
      setSaving(false)
    }
  }

  async function createBranch() {
    setError('')
    try {
      await api.createBranch(owner, name, branchName.trim(), branchRef.trim() || undefined)
      setBranchName('')
      setBranchRef('')
      void loadRefs()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function deleteBranch(branch: string) {
    setError('')
    try {
      await api.deleteBranch(owner, name, branch)
      void loadRefs()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function createTag() {
    setError('')
    try {
      await api.createTag(owner, name, tagName.trim(), tagRef.trim() || undefined)
      setTagName('')
      setTagRef('')
      void loadRefs()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function deleteTag(tag: string) {
    setError('')
    try {
      await api.deleteTag(owner, name, tag)
      void loadRefs()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function moveRepository() {
    if (!targetOwner) return
    setError('')
    setMoving(true)
    try {
      await api.updateRepo(owner, name, { new_owner: targetOwner })
      setConfirmMove(false)
      // The repository now lives under the new namespace: follow it there.
      // The clone URLs are derived from the location, so they change with it.
      navigate(`/${targetOwner}/${name}/settings`)
    } catch (err) {
      setError(translateError(err))
      setMoving(false)
      setConfirmMove(false)
    }
  }

  async function deleteRepository() {
    setError('')
    setDeleting(true)
    try {
      await api.deleteRepo(owner, name)
      navigate('/')
    } catch (err) {
      setError(translateError(err))
      setDeleting(false)
      setConfirmDelete(false)
    }
  }

  const cloneHttps = repo?.clone_url_https
    ? repo.clone_url_https.startsWith('/')
      ? `${window.location.origin}${repo.clone_url_https}`
      : repo.clone_url_https
    : ''
  const cloneSsh = repo?.clone_url_ssh ?? ''

  return (
    <div className="repo-settings">
      <PageHeader
        icon={<SettingsIcon size={18} />}
        title={
          <>
            {owner} <span style={{ color: 'var(--fg-subtle)', fontWeight: 400 }}>/</span> {name}
          </>
        }
        subtitle={t('repoSettings.title')}
        actions={
          <button className="btn btn-ghost" onClick={onBack}>
            {t('common.back')}
          </button>
        }
      />

      {error && <Alert type="error">{error}</Alert>}
      {saved && <Alert type="success">{t('repoSettings.saved')}</Alert>}


      <div className="repo-settings-layout">
        {/* Left column: section navigation, GitHub-style. */}
        <nav className="repo-settings-nav" aria-label={t('repoSettings.title')}>
          <ul>
            {SECTIONS.map((s) => (
              <li key={s}>
                <button
                  className={`repo-settings-nav-item${active === s ? ' active' : ''}`}
                  onClick={() => onSectionChange(s)}
                  aria-current={active === s ? 'page' : undefined}
                >
                  <SectionIcon section={s} />
                  {t(`repoSettings.${s}`)}
                </button>
              </li>
            ))}
          </ul>
        </nav>

        <div className="repo-settings-main">
        {active === 'general' && (
          <>
            <div className="settings-section">
              <h3>{t('repoSettings.general')}</h3>
              <p>{t('repoSettings.generalDesc')}</p>

              <label className="settings-field">
                <span>{t('repoSettings.repoName')}</span>
                <input
                  className="input"
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                />
              </label>

              <label className="settings-field">
                <span>{t('repoSettings.description')}</span>
                <textarea
                  className="input"
                  rows={3}
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                />
              </label>

              <label className="settings-field settings-field-inline">
                <span>{t('repoSettings.visibility')}</span>
                <select
                  className="select"
                  value={isPrivate ? 'private' : 'public'}
                  onChange={(e) => setIsPrivate(e.target.value === 'private')}
                >
                  <option value="public">{t('repoSettings.public')}</option>
                  <option value="private">{t('repoSettings.private')}</option>
                </select>
              </label>

              <div className="settings-field">
                <span>{t('repoSettings.defaultBranch')}</span>
                <span className="settings-static">{repo?.default_branch ?? '—'}</span>
              </div>

              {cloneHttps && (
                <div className="settings-field">
                  <span>{t('repoSettings.cloneUrls')}</span>
                  <div className="settings-clone-urls">
                    <code>{cloneHttps}</code>
                    {cloneSsh && <code>{cloneSsh}</code>}
                  </div>
                  <p className="settings-hint">{t('repoSettings.cloneUrlsHint')}</p>
                </div>
              )}

              {/* Move the repository to another namespace. The clone URLs are
                  derived from the location, so they change with the move. */}
              {groups.length > 0 && (
                <div className="settings-field">
                  <span>{t('repoSettings.moveTo')}</span>
                  <div className="settings-move-row">
                    <select
                      className="select"
                      value={targetOwner}
                      onChange={(e) => setTargetOwner(e.target.value)}
                    >
                      <option value="">{t('repoSettings.pickGroup')}</option>
                      {groups.map((g) => (
                        <option key={g.id} value={g.full_path}>
                          {g.full_path}
                        </option>
                      ))}
                    </select>
                    <button
                      className="btn btn-sm"
                      disabled={!targetOwner}
                      onClick={() => setConfirmMove(true)}
                    >
                      {t('repoSettings.moveRepo')}
                    </button>
                  </div>
                  <p className="settings-hint">{t('repoSettings.moveDesc')}</p>
                </div>
              )}

            <button className="btn btn-primary" disabled={saving} onClick={() => void saveGeneral()}>
              {saving ? t('repoSettings.saving') : t('repoSettings.save')}
            </button>
          </div>

          <div className="settings-section settings-danger">
            <h3>{t('repoSettings.danger')}</h3>
            <p>{t('repoSettings.dangerDesc')}</p>
            <button className="btn btn-danger" onClick={() => setConfirmDelete(true)}>
              {t('repoSettings.deleteRepo')}
            </button>
          </div>
        </>
      )}

      {active === 'branches' && (
        <div className="settings-section">
          <h3>{t('repoSettings.branches')}</h3>
          <p>{t('repoSettings.branchesDesc')}</p>
          <div className="settings-create-row">
            <input
              className="input"
              placeholder={t('repo.createBranchPlaceholder')}
              value={branchName}
              onChange={(e) => setBranchName(e.target.value)}
            />
            <input
              className="input"
              placeholder={t('repo.refFromPlaceholder')}
              value={branchRef}
              onChange={(e) => setBranchRef(e.target.value)}
            />
            <button
              className="btn btn-primary btn-sm"
              disabled={!branchName.trim()}
              onClick={() => void createBranch()}
            >
              <PlusIcon size={14} />
              {t('repo.createBranchBtn')}
            </button>
          </div>
          <ul className="settings-ref-list">
            {branches.map((b) => (
              <li key={b}>
                <span className="settings-ref-name">
                  <GitBranchIcon size={14} />
                  {b}
                </span>
                {b === repo?.default_branch && (
                  <span className="ref-selector-badge">{t('repo.ref.default')}</span>
                )}
                <button
                  className="btn btn-ghost btn-xs"
                  title={t('repo.deleteBranchTitle')}
                  onClick={() => void deleteBranch(b)}
                >
                  {t('repo.deleteBranchTitle')}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {active === 'tags' && (
        <div className="settings-section">
          <h3>{t('repoSettings.tags')}</h3>
          <p>{t('repoSettings.tagsDesc')}</p>
          <div className="settings-create-row">
            <input
              className="input"
              placeholder={t('repo.createTagPlaceholder')}
              value={tagName}
              onChange={(e) => setTagName(e.target.value)}
            />
            <input
              className="input"
              placeholder={t('repo.refFromPlaceholder')}
              value={tagRef}
              onChange={(e) => setTagRef(e.target.value)}
            />
            <button
              className="btn btn-primary btn-sm"
              disabled={!tagName.trim()}
              onClick={() => void createTag()}
            >
              <PlusIcon size={14} />
              {t('repo.createTagBtn')}
            </button>
          </div>
          <ul className="settings-ref-list">
            {tags.map((tag) => (
              <li key={tag}>
                <span className="settings-ref-name">
                  <TagIcon size={14} />
                  {tag}
                </span>
                <button
                  className="btn btn-ghost btn-xs"
                  title={t('repo.deleteTagTitle')}
                  onClick={() => void deleteTag(tag)}
                >
                  {t('repo.deleteTagTitle')}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {active === 'pages' && <PagesPanel owner={owner} name={name} />}
        </div>
      </div>

      {confirmMove && (
        <ConfirmDialog
          title={t('repoSettings.moveRepo')}
          message={t('repoSettings.moveRepoConfirm', {
            from: `${owner}/${name}`,
            to: `${targetOwner}/${name}`,
          })}
          confirmLabel={t('repoSettings.moveRepo')}
          busy={moving}
          onCancel={() => setConfirmMove(false)}
          onConfirm={() => void moveRepository()}
        />
      )}

      {confirmDelete && (
        <ConfirmDialog
          title={t('repoSettings.deleteRepo')}
          message={t('repoSettings.deleteRepoConfirm')}
          confirmLabel={t('repoSettings.deleteRepo')}
          busy={deleting}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void deleteRepository()}
        />
      )}
    </div>
  )
}
