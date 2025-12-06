import { Link } from '@tanstack/react-router'
import { useAuth } from '../auth/AuthContext'

export function AppLayout({ children }: { children: React.ReactNode }) {
  const { state, logout } = useAuth()

  return (
    <div className="min-h-screen flex flex-col bg-gray-100 text-gray-900">
      <header className="h-14 bg-white border-b shadow-sm flex items-center px-6 justify-between">
        <div className="flex items-center gap-4">
          <Link to="/">
            <h1 className="text-lg font-semibold">Dromio Scheduler</h1>
          </Link>
          {state.status === 'authenticated' && (
            <>
              <Link to="/jobs" className="hover:text-blue-600">
                Jobs
              </Link>
              <Link to="/runs" className="hover:text-blue-600">
                Runs
              </Link>
              <Link to="/workers" className="hover:text-blue-600">
                Workers
              </Link>
              {/* <Link to="/users" className="hover:text-blue-600">Users</Link> */}
            </>
          )}
        </div>

        <div>
          {state.status === 'authenticated' && (
            <div className="flex items-center gap-3">
              <span className="text-sm text-gray-700">
                {state.user.username} ({state.user.role})
              </span>
              <button
                onClick={() => logout()}
                className="text-sm text-gray-600 hover:text-red-600"
              >
                Logout
              </button>
            </div>
          )}
        </div>
      </header>

      <main className="flex-1 px-8 py-6">
        <div className="max-w-6xl mx-auto">{children}</div>
      </main>
    </div>
  )
}
