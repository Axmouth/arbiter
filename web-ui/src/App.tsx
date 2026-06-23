import { RouterProvider } from '@tanstack/react-router'
import { router } from './router'
import { AuthProvider } from './auth/AuthContext'
import { useAuth } from './auth/useAuth'

function InnerApp() {
  const auth = useAuth()
  return <RouterProvider router={router} context={{ auth }} />
}

export default function App() {
  return (
    <AuthProvider>
      <InnerApp />
    </AuthProvider>
  )
}
