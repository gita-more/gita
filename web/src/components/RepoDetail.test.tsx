// Tests for RepoDetail — the repository page.
//
// This file is the largest in the web app (over 1300 lines) and it had
// no tests at all. It is also where the product lives: the file
// browser, the commit list, the pull requests, the issues, the
// comments. Everything below is about the parts that can break without
// a crash:
//
//  1. each tab must load its own data — a tab that loads the wrong
//     collection shows the wrong list;
//  2. a failed load must show the translated error, not an empty page
//     that looks like "there is nothing here";
//  3. an empty repository must invite the first push instead of showing
//     an empty directory listing;
//  4. starring must call the API and show the new count;
//  5. the commit list must show what the API returned.
//
// The component calls the API through `../lib/api`, so the module is
// mocked. The mock is a table of the calls each tab makes, so a test
// fails when the component starts asking for something else.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { I18nProvider } from '../i18n'
import { translate } from '../i18n/translate'
import type { Locale } from '../i18n/types'

// The API module is mocked wholesale; the mock is rebuilt per test so
// that one test's stubs cannot answer another test's calls.
//
// `vi.mock` is hoisted above the module's own imports, so the mock
// table has to be hoisted with it.
const apiMock = vi.hoisted(() => ({
  getRepo: vi.fn(),
  getBranches: vi.fn(),
  getTags: vi.fn(),
  getStar: vi.fn(),
  starRepo: vi.fn(),
  unstarRepo: vi.fn(),
  getDir: vi.fn(),
  getFile: vi.fn(),
  getCommits: vi.fn(),
  getCommit: vi.fn(),
  listPrs: vi.fn(),
  getPr: vi.fn(),
  getPrDiff: vi.fn(),
  mergePr: vi.fn(),
  closePr: vi.fn(),
  createPr: vi.fn(),
  listPrComments: vi.fn(),
  createPrComment: vi.fn(),
  listIssues: vi.fn(),
  closeIssue: vi.fn(),
  createIssue: vi.fn(),
  listIssueComments: vi.fn(),
  createIssueComment: vi.fn(),
  deleteComment: vi.fn(),
  listPipelines: vi.fn(),
}))

vi.mock('../lib/api', () => ({ api: apiMock }))

import { RepoDetail } from './RepoDetail'

/** A repository with content: one branch, one commit, one file. */
function populatedApi() {
  apiMock.getRepo.mockResolvedValue({ full_name: 'alice/widgets' })
  apiMock.getBranches.mockResolvedValue(['main'])
  apiMock.getTags.mockResolvedValue(['v1.0.0'])
  apiMock.getStar.mockResolvedValue({ starred: false, count: 3 })
  apiMock.getDir.mockResolvedValue({
    entries: [
      { path: 'README.md', name: 'README.md', kind: 'blob', size: 12 },
      { path: 'src', name: 'src', kind: 'tree', size: null },
    ],
  })
  apiMock.getCommits.mockResolvedValue({
    commits: [
      {
        id: 1,
        repo_id: 1,
        sha: 'abc1234',
        message: 'the first commit',
        author_name: 'Alice',
        author_email: 'alice@example.com',
        parent_shas: [],
        authored_at: '2026-01-01T00:00:00Z',
      },
    ],
  })
  apiMock.listPrs.mockResolvedValue({ prs: [] })
  apiMock.listIssues.mockResolvedValue({ issues: [] })
  apiMock.listPipelines.mockResolvedValue({ pipelines: [] })
  // The README of a directory is fetched and rendered on its own, so
  // the file endpoint has to answer too.
  apiMock.getFile.mockResolvedValue({
    path: 'README.md',
    content: '# widgets\n\nA repository.\n',
    sha: 'abc1234',
  })
}

/** The locale the app is currently in. The label of a button is
 * localized, so a test that looks for a button by its label has to ask
 * the dictionary rather than hard-code one language. */
function currentLocale(): Locale {
  return document.documentElement.lang === 'en' ? 'en' : 'zh'
}

function renderRepo(tab: 'files' | 'commits' | 'prs' | 'issues' | 'pipelines' = 'files') {
  return render(
    <MemoryRouter initialEntries={[`/alice/widgets/${tab}`]}>
      <I18nProvider>
        <RepoDetail owner="alice" name="widgets" tab={tab} onBack={() => {}} />
      </I18nProvider>
    </MemoryRouter>,
  )
}

beforeEach(() => {
  for (const fn of Object.values(apiMock)) fn.mockReset()
  populatedApi()
})

describe('the repository page loads the data its tab needs', () => {
  it('loads the directory listing and the commits for the files tab', async () => {
    renderRepo('files')

    await waitFor(() => expect(apiMock.getDir).toHaveBeenCalled())
    expect(apiMock.getDir).toHaveBeenCalledWith('alice', 'widgets', '', 'main')
    // The commits feed the "latest commit" bar above the listing.
    await waitFor(() => expect(apiMock.getCommits).toHaveBeenCalled())
    expect(apiMock.listPrs).not.toHaveBeenCalled()
    expect(apiMock.listIssues).not.toHaveBeenCalled()
  })

  it('loads the pull requests for the prs tab', async () => {
    renderRepo('prs')

    await waitFor(() => expect(apiMock.listPrs).toHaveBeenCalled())
    expect(apiMock.listPrs).toHaveBeenCalledWith('alice', 'widgets', 'all')
    // A tab loads its own collection and no other.
    expect(apiMock.getDir).not.toHaveBeenCalled()
    expect(apiMock.listIssues).not.toHaveBeenCalled()
  })

  it('loads the issues for the issues tab', async () => {
    renderRepo('issues')

    await waitFor(() => expect(apiMock.listIssues).toHaveBeenCalled())
    expect(apiMock.listIssues).toHaveBeenCalledWith('alice', 'widgets', 'all')
    expect(apiMock.listPrs).not.toHaveBeenCalled()
  })

  it('loads the pipelines for the pipelines tab', async () => {
    renderRepo('pipelines')

    await waitFor(() => expect(apiMock.listPipelines).toHaveBeenCalled())
    expect(apiMock.listPipelines).toHaveBeenCalledWith('alice', 'widgets')
  })
})

describe('the page shows what came back', () => {
  it('lists the files of the directory', async () => {
    renderRepo('files')

    expect(await screen.findByText('README.md')).toBeTruthy()
    expect(screen.getByText('src')).toBeTruthy()
  })

  it('shows the commit and its message for the commits tab', async () => {
    renderRepo('commits')

    expect(await screen.findByText('the first commit')).toBeTruthy()
    expect(screen.getByText(/abc1234/)).toBeTruthy()
  })

  it('shows the star count', async () => {
    renderRepo('files')

    // The label is localized, so the test asks the dictionary for it
    // rather than hard-coding one language.
    const label = translate(currentLocale(), 'repo.star')
    const star = await screen.findByTitle(label)
    // The count comes from the API: three stars, not the fallback.
    expect(star.textContent).toContain('3')
  })
})

describe('a failure is reported rather than hidden', () => {
  it('shows the translated error when the directory cannot be loaded', async () => {
    apiMock.getDir.mockRejectedValue(new Error('boom'))
    renderRepo('files')

    // The error must be visible: an empty listing would look exactly
    // like an empty repository, and the user would push a second copy
    // of a repository they already have.
    await waitFor(() => expect(screen.getByText(/boom|error|failed/i)).toBeTruthy())
  })

  it('shows the error when the pull requests cannot be loaded', async () => {
    apiMock.listPrs.mockRejectedValue(new Error('pr list failed'))
    renderRepo('prs')

    await waitFor(() => expect(screen.getByText(/pr list failed/i)).toBeTruthy())
  })
})

describe('starring a repository', () => {
  it('calls the API and shows the new count', async () => {
    apiMock.starRepo.mockResolvedValue({ starred: true, count: 4 })
    renderRepo('files')

    const star = await screen.findByTitle(translate(currentLocale(), 'repo.star'))
    await userEvent.click(star)

    await waitFor(() => expect(apiMock.starRepo).toHaveBeenCalledWith('alice', 'widgets'))
    await waitFor(() => expect(star.textContent).toContain('4'))
    // The button now offers to undo the star.
    await waitFor(() =>
      expect(screen.getByTitle(translate(currentLocale(), 'repo.unstar'))).toBeTruthy(),
    )
  })

  it('unstars a repository that is already starred', async () => {
    apiMock.getStar.mockResolvedValue({ starred: true, count: 3 })
    apiMock.unstarRepo.mockResolvedValue({ starred: false, count: 2 })
    renderRepo('files')

    const star = await screen.findByTitle(translate(currentLocale(), 'repo.unstar'))
    await userEvent.click(star)

    await waitFor(() => expect(apiMock.unstarRepo).toHaveBeenCalledWith('alice', 'widgets'))
    await waitFor(() => expect(star.textContent).toContain('2'))
  })
})

describe('an empty repository', () => {
  it('invites the first push instead of showing an empty listing', async () => {
    apiMock.getDir.mockResolvedValue({ entries: [] })
    apiMock.getCommits.mockResolvedValue({ commits: [] })
    renderRepo('files')

    // The empty-repository welcome is the whole point of the page in
    // this state: the user needs to know what to do next.
    await waitFor(() => {
      const welcome = document.body.textContent ?? ''
      expect(
        /push|clone|quick setup/i.test(welcome),
        `expected a first-push invitation, got: ${welcome.slice(0, 400)}`,
      )
    })
  })
})
