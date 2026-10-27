import { useEffect, useState } from 'react'
import { marked } from 'marked'
import DOMPurify from 'dompurify'
import { api } from '../lib/api'
import { useI18n } from '../i18n'
import { BookIcon } from './Icons'

/**
 * GitHub-style README block rendered under the directory listing: a boxed
 * header plus sanitized markdown. The README path comes from the parent's
 * directory listing so we only fetch when one is actually present.
 */
export function ReadmePanel({
  owner,
  name,
  readmePath,
  rev,
}: {
  owner: string
  name: string
  readmePath: string | null
  rev: string
}) {
  const { t } = useI18n()
  const [content, setContent] = useState<string | null>(null)

  useEffect(() => {
    if (!readmePath) {
      setContent(null)
      return
    }
    let cancelled = false
    setContent(null)
    api
      .getFile(owner, name, readmePath, rev)
      .then((file) => {
        if (!cancelled) setContent(file.content)
      })
      .catch(() => {
        // A missing README is not an error worth a banner — render nothing.
        if (!cancelled) setContent(null)
      })
    return () => {
      cancelled = true
    }
  }, [owner, name, readmePath, rev])

  if (!readmePath || content === null) return null

  // Sanitize before injecting: markdown may contain raw HTML.
  const html = DOMPurify.sanitize(marked.parse(content, { async: false }))

  return (
    <div className="box readme-box">
      <div className="box-header">
        <span className="readme-title">
          <BookIcon size={15} />
          {t('repo.code.readmeTitle')}
        </span>
      </div>
      <div className="box-body markdown-body" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  )
}
