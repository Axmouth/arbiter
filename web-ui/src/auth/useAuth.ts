import { createContext, useContext } from 'react'
import type { User } from '../backend-types'

export type AuthState =
  | { status: 'loading'; user: null }
  | { status: 'unauthenticated'; user: null }
  | { status: 'authenticated'; user: User }

export type AuthContextValue = {
  state: AuthState
  login: (username: string, password: string) => Promise<void>
  logout: () => Promise<void>
}

export const AuthContext = createContext<AuthContextValue | undefined>(undefined)

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used inside AuthProvider')
  return ctx
}
