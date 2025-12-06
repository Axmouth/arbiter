import { createRouter, RouterProvider } from '@tanstack/react-router'
import { jobsRoute } from './routes/jobs'
import { runsRoute } from './routes/runs'
import { rootRoute } from './routes/root'
import { workersRoute } from './routes/workers'
import { loginRoute } from './routes/login'
import { useAuth } from './auth/AuthContext'
import { homeRoute } from './routes/home'

const routeTree = rootRoute.addChildren([
  homeRoute,
  loginRoute,
  jobsRoute,
  runsRoute,
  workersRoute,
])

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

// eslint-disable-next-line react-refresh/only-export-components
export const router = createRouter({
  routeTree,
  context: {
    auth: undefined!,
  },
})

export function AppRouter() {
  const auth = useAuth()

  return <RouterProvider router={router} context={{ auth }} />
}
