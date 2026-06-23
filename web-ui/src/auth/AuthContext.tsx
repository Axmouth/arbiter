import { useEffect, useState } from 'react'
import {
  me as fetchMe,
  login as apiLogin,
  logout as apiLogout,
} from '../api/auth'
import { redirect } from '@tanstack/react-router'
import { AuthContext, type AuthState } from './useAuth'

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [state, setState] = useState<AuthState>({
    status: 'loading',
    user: null,
  })

  useEffect(() => {
    fetchMe()
      .then((user) => setState({ status: 'authenticated', user }))
      .catch(() => setState({ status: 'unauthenticated', user: null }))
  }, [])

  async function login(username: string, password: string) {
    const user = await apiLogin(username, password)
    setState({ status: 'authenticated', user })
  }

  async function logout() {
    await apiLogout()
    setState({ status: 'unauthenticated', user: null })

    throw redirect({ to: '/login' })
  }

  return (
    <AuthContext.Provider value={{ state, login, logout }}>
      {children}
    </AuthContext.Provider>
  )
}
