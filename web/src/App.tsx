import { useEffect, useState } from 'react'
import { LoginPage } from './components/LoginPage'
import { Projects } from './components/Projects'
import { Groups } from './components/Groups'
import { Issues } from './components/Issues'
import { GroupDetail } from './components/GroupDetail'
import { RepoDetail } from './components/RepoDetail'
import { Settings } from './components/Settings'
import { useAuth } from './hooks/useAuth'
import { setOpenSettings } from './components/Layout'

/** Three-level navigation hierarchy, plus settings. */
type View =
  | { kind: 'global' }                          // global Pages: Projects/Groups/Issues
  | { kind: 'group'; group: string }            // one specific group
  | { kind: 'repo'; owner: string; name: string }
  | { kind: 'settings' }                        // user settings page

export default function App() {
  const { user } = useAuth()
  const [view, setView] = useState<View>({ kind: 'global' })
  const [globalTab, setGlobalTab] = useState<'projects' | 'groups' | 'issues'>('projects')

  // Register the "Settings" entry point used by the user menu.
  useEffect(() => {
    setOpenSettings(() => setView({ kind: 'settings' }))
    return () => setOpenSettings(null)
  }, [])

  if (!user) {
    return <LoginPage onDone={() => setGlobalTab('projects')} />
  }

  if (view.kind === 'settings') {
    return <Settings onBack={() => setView({ kind: 'global' })} />
  }

  if (view.kind === 'repo') {
    return (
      <RepoDetail
        owner={view.owner}
        name={view.name}
        onBack={() => setView({ kind: 'global' })}
      />
    )
  }

  if (view.kind === 'group') {
    return (
      <GroupDetail
        group={view.group}
        onBack={() => setView({ kind: 'global' })}
        onOpenRepo={(owner, name) => setView({ kind: 'repo', owner, name })}
      />
    )
  }

  // Global view: pick Projects / Groups / Issues tab.
  if (globalTab === 'projects') {
    return (
      <Projects
        onOpenRepo={(owner, name) => setView({ kind: 'repo', owner, name })}
        onNavigate={setGlobalTab}
        activeTab={globalTab}
      />
    )
  }
  if (globalTab === 'groups') {
    return (
      <Groups
        onOpenGroup={(group) => setView({ kind: 'group', group })}
        onNavigate={setGlobalTab}
        activeTab={globalTab}
      />
    )
  }
  return (
    <Issues
      onOpenRepo={(owner, name) => setView({ kind: 'repo', owner, name })}
      onNavigate={setGlobalTab}
      activeTab={globalTab}
    />
  )
}