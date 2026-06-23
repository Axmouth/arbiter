import { createRoute } from '@tanstack/react-router'
import { rootRoute } from './root'
import { TenantsPage } from '../pages/TenantsPage'

export const tenantsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/tenants',
  component: TenantsPage,
})
