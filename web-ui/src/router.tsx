import { createRouter } from '@tanstack/react-router'
import { jobsRoute } from './routes/jobs'
import { runsRoute } from './routes/runs'
import { rootRoute } from './routes/root'
import { workersRoute } from './routes/workers'
import { loginRoute } from './routes/login'
import { homeRoute } from './routes/home'
import { secretsRoute } from './routes/secrets'
import { configsRoute } from './routes/configs'
import { tenantsRoute } from './routes/tenants'
import { usersRoute } from './routes/users'

const routeTree = rootRoute.addChildren([
  homeRoute,
  loginRoute,
  jobsRoute,
  runsRoute,
  workersRoute,
  secretsRoute,
  configsRoute,
  tenantsRoute,
  usersRoute,
])

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

export const router = createRouter({
  routeTree,
  context: {
    auth: undefined!,
  },
})
