import { Link } from '@tanstack/react-router'
import { useAuth } from '../auth/AuthContext'
import DarkmodeToggle from '../components/DarkmodeToggle'
import { useEffect, useState } from 'react'
import { ConfigProvider, theme as antdTheme } from 'antd'

export function AppLayout({ children }: { children: React.ReactNode }) {
  const { state, logout } = useAuth()
  const [isDarkMode, setIsDarkMode] = useState(() => {
    // Get the user's preference from local storage if it exists
    const savedTheme = localStorage.getItem('theme')
    return savedTheme === 'dark' ? true : false
  })

  useEffect(() => {
    // Save the user's preference in local storage
    localStorage.setItem('theme', isDarkMode ? 'dark' : 'light')
    document.body.setAttribute('data-theme', isDarkMode ? 'dark' : 'light')
  }, [isDarkMode])

  // Function to toggle the theme
  const toggleTheme = () => {
    setIsDarkMode(!isDarkMode)
  }

  return (
    <div className="min-h-screen flex flex-col bg-(--bg-app) text-(--text-primary)">
      <header className="h-14 bg-(--bg-header) border-b border-(--border-subtle) shadow-sm flex items-center px-6 justify-between">
        <div className="flex items-center gap-4">
          <Link to="/">
            <h1 className="text-lg font-semibold">Dromio Scheduler</h1>
          </Link>
          {state.status === 'authenticated' && (
            <>
              <Link to="/jobs" className="hover:text-(--text-accent)">
                Jobs
              </Link>
              <Link to="/runs" className="hover:text-(--text-accent)">
                Runs
              </Link>
              <Link to="/workers" className="hover:text-(--text-accent)">
                Workers
              </Link>
              {/* <Link to="/users" className="hover:text-blue-600">Users</Link> */}
            </>
          )}
        </div>

        <div className="flex items-center gap-3">
          <DarkmodeToggle onThemeToggle={toggleTheme} isDarkMode={isDarkMode} />
          {state.status === 'authenticated' && (
            <div className="flex items-center gap-3">
              <span className="text-sm text-(--text-secondary)">
                {state.user.username} ({state.user.role})
              </span>
              <button
                onClick={() => logout()}
                className="text-sm text-(--text-secondary) hover:text-(--text-danger)"
              >
                Logout
              </button>
            </div>
          )}
        </div>
      </header>

      <main className="flex-1 px-8 py-6">
        <div className="max-w-6xl mx-auto">
          <ConfigProvider
            theme={{
              algorithm: isDarkMode
                ? [antdTheme.darkAlgorithm]
                : [antdTheme.defaultAlgorithm],
            }}
          >
            {children}
          </ConfigProvider>
        </div>
      </main>
    </div>
  )
}
