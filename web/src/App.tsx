import { useState } from 'react'
import { LoginPage } from './components/LoginPage'
import { RepoList } from './components/RepoList'
import { RepoDetail } from './components/RepoDetail'
import { useAuth } from './hooks/useAuth'

export default function App() {
  const { user } = useAuth()
  const [view, setView] = useState<{ owner: string; name: string } | null>(null)

  if (!user) {
    return <LoginPage onDone={() => {}} />
  }

  if (view) {
    return (
      <RepoDetail
        owner={view.owner}
        name={view.name}
        onBack={() => setView(null)}
      />
    )
  }

  return <RepoList onOpen={(owner, name) => setView({ owner, name })} />
}