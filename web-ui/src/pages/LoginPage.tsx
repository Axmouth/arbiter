import { type FormEvent, useEffect, useState } from 'react'
import { useAuth } from '../auth/AuthContext'
import { useNavigate, useRouter } from '@tanstack/react-router'
import type { ApiResponse } from '../backend-types'

function usePreviousLocation() {
  const router = useRouter()
  const [previousLocation, setPreviousLocation] = useState<string>('/')
  useEffect(() => {
    return router.subscribe('onResolved', ({ fromLocation }) => {
      let target = fromLocation?.href ?? '/'
      if (target.startsWith('/login')) {
        target = '/'
      }
      setPreviousLocation(target)
    })
  }, [router])
  return previousLocation
}

export function LoginPage() {
  const navigate = useNavigate()
  const previousLocation = usePreviousLocation()
  const { state, login } = useAuth()
  const router = useRouter()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    setLoading(true)
    try {
      await login(username, password)
      router.navigate({ to: '/' }) // jobs
    } catch (err) {
      const resp: ApiResponse<null> = JSON.parse((err as Error).message)
      if (resp.status === 'error') {
        setError(resp.message)
      }
    } finally {
      setLoading(false)
    }
  }

  if (state.status === 'authenticated') {
    // Optional: if already logged in, redirect or show "already logged in"
    navigate({ to: previousLocation })
  }

  return (
    <div
      className="
      min-h-screen flex items-center justify-center
      bg-(--bg-app) text-(--text-primary)
    "
    >
      <form
        onSubmit={onSubmit}
        className="
          bg-(--bg-surface-dialog)
          shadow rounded-lg p-6 w-full max-w-sm space-y-4
          border border-(--border-subtle)
        "
      >
        <h1 className="text-xl font-semibold text-center">Arbiter Login</h1>

        {error && <p className="text-sm text-(--text-danger)">{error}</p>}

        <div>
          <label className="block text-sm font-medium text-(--text-primary)">
            Username
          </label>
          <input
            className="
              mt-1 w-full rounded px-3 py-2
              border border-(--border-color)
              bg-(--bg-app) text-(--text-primary)
            "
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-(--text-primary)">
            Password
          </label>
          <input
            type="password"
            className="
              mt-1 w-full rounded px-3 py-2
              border border-(--border-color)
              bg-(--bg-app) text-(--text-primary)
            "
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
          />
        </div>

        <button
          type="submit"
          disabled={loading}
          className="
            w-full rounded py-2 disabled:opacity-50
            bg-(--bg-btn-primary) text-(--text-inverse)
            hover:bg-(--bg-btn-primary-hover)
          "
        >
          {loading ? 'Logging in…' : 'Login'}
        </button>
      </form>
    </div>
  )
}
