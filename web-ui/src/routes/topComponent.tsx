import { Outlet, useNavigate } from '@tanstack/react-router'
import { AppLayout } from '../layout/AppLayout'
import { publicRoutes } from '../router/publicRoutes'
import { useAuth } from '../auth/AuthContext'

export const TopComponent = () => {
  const auth = useAuth()
  const navigate = useNavigate()
  const path = location.pathname

  if (
    auth?.state?.status === 'unauthenticated' &&
    ![...publicRoutes].some((prefix) => path.startsWith(prefix))
  ) {
    console.log('Unauthanticated, redirecting..')
    navigate({
      to: '/login',
      search: {
        redirect: path, // return-to
      },
    })
  }

  return (
    <AppLayout>
      <Outlet />
    </AppLayout>
  )
}
