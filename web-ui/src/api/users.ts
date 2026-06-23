import type {
  ApiResponse,
  CreateUserRequest,
  UpdateUserRequest,
  User,
} from '../backend-types'

// User management lives under /api (the auth router), not /api/v1, so it does not go
// through the shared v1 `api()` client.
const apiBaseUrl =
  import.meta.env.MODE === 'development1' ? 'http://localhost:8080' : ''

async function authApi<T>(path: string, options: RequestInit = {}): Promise<T> {
  const res = await fetch(`${apiBaseUrl}/api${path}`, {
    credentials: 'include',
    headers: { 'Content-Type': 'application/json' },
    ...options,
  })
  const json: ApiResponse<T> = await res.json()
  if (json.status === 'error') {
    throw new Error(json.message)
  }
  return json.data as T
}

export function fetchUsers(): Promise<User[]> {
  return authApi<User[]>('/users')
}

export function createUser(req: CreateUserRequest): Promise<User> {
  return authApi<User>('/users', {
    method: 'POST',
    body: JSON.stringify(req),
  })
}

export function updateUser(id: string, req: UpdateUserRequest): Promise<User> {
  return authApi<User>(`/users/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(req),
  })
}

export function deleteUser(id: string): Promise<void> {
  return authApi<void>(`/users/${id}`, { method: 'DELETE' })
}
