import type { ApiResponse, User } from '../backend-types'

const apiBaseUrl =
  import.meta.env.MODE === 'development1' ? 'http://localhost:8080' : ''

export async function login(username: string, password: string): Promise<User> {
  const res = await fetch(`${apiBaseUrl}/api/login`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  })

  if (!res.ok) {
    const msg = await res.text().catch(() => 'Login failed')
    throw new Error(msg || 'Login failed')
  }

  // if your endpoint returns user info, parse it. Otherwise call `me()` next.
  return await me()
}

export async function logout(): Promise<void> {
  await fetch(`${apiBaseUrl}/api/logout`, {
    method: 'POST',
    credentials: 'include',
  })
}

export async function me(): Promise<User> {
  const res = await fetch(`${apiBaseUrl}/api/me`, {
    method: 'GET',
    credentials: 'include',
  })

  if (res.status === 401) {
    throw new Error('unauthenticated')
  }

  const json: ApiResponse<User> = await res.json()

  if (!res.ok || json.status != 'ok') {
    const msg = await res.text().catch(() => 'Failed to load session')
    throw new Error(msg)
  }

  return await json.data
}
