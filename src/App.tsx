import { TitleBar } from './components/TitleBar'
import { Welcome } from './pages/Welcome'
import { Docker } from './pages/Docker'
import { Key } from './pages/Key'
import { Model } from './pages/Model'
import { Pull } from './pages/Pull'
import { Start } from './pages/Start'
import { Done } from './pages/Done'
import { Dashboard } from './pages/Dashboard'
import { Settings } from './pages/Settings'
import { Logs } from './pages/Logs'
import { Feedback } from './pages/Feedback'
import { ErrorPage } from './pages/ErrorPage'
import { pageOf } from './state/machine'
import { useStore } from './state/context'
import { useAsync } from './lib/useAsync'
import * as ipc from './lib/ipc'

export function App() {
  const { overlay } = useStore()
  const app = useAsync(() => ipc.appInfo(), [])

  return (
    <div className="flex h-full flex-col overflow-hidden bg-window">
      <TitleBar info={app.data} />
      <main className="flex min-h-0 flex-1 flex-col">
        {overlay ? <Overlay which={overlay} /> : <Page />}
      </main>
    </div>
  )
}

function Page() {
  const { state } = useStore()
  switch (pageOf(state)) {
    case 'welcome':
      return <Welcome />
    case 'docker':
      return <Docker />
    case 'key':
      return <Key />
    case 'model':
      return <Model />
    case 'pull':
      return <Pull />
    case 'start':
      return <Start />
    case 'done':
      return <Done />
    case 'dashboard':
      return <Dashboard />
    case 'error':
      return <ErrorPage />
  }
}

function Overlay({ which }: { which: 'settings' | 'logs' | 'feedback' }) {
  const { state } = useStore()
  if (which === 'settings') return <Settings />
  if (which === 'logs') return <Logs />
  return <Feedback errorCode={state.name === 'Error' ? state.code : undefined} />
}
