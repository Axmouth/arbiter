import { createRoute } from '@tanstack/react-router'
import { rootRoute } from './root'
import { DbConfigsPage } from '../pages/DbConfigsPage'

export const configsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/db-configs',
  component: DbConfigsPage,
})
