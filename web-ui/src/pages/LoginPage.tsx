import { type FormEvent, useEffect, useState } from 'react'
import { useAuth } from '../auth/AuthContext'
import { useNavigate, useRouter } from '@tanstack/react-router'

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
      setError((err as Error).message || 'Login failed')
    } finally {
      setLoading(false)
    }
  }

  if (state.status === 'authenticated') {
    // Optional: if already logged in, redirect or show "already logged in"
    navigate({ to: previousLocation })
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-gray-100">
      <form
        onSubmit={onSubmit}
        className="bg-white shadow rounded-lg p-6 w-full max-w-sm space-y-4"
      >
        <h1 className="text-xl font-semibold text-center">Dromio Login</h1>

        {error && <p className="text-sm text-red-600">{error}</p>}

        <div>
          <label className="block text-sm font-medium">Username</label>
          <input
            className="mt-1 w-full border rounded px-3 py-2"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
          />
        </div>

        <div>
          <label className="block text-sm font-medium">Password</label>
          <input
            type="password"
            className="mt-1 w-full border rounded px-3 py-2"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
          />
        </div>

        <button
          type="submit"
          disabled={loading}
          className="w-full bg-blue-600 text-white rounded py-2 hover:bg-blue-700 disabled:opacity-50"
        >
          {loading ? 'Logging in…' : 'Login'}
        </button>
      </form>
    </div>
  )
}
