import { createRoute } from '@tanstack/react-router'
import { rootRoute } from './root'
import { SecretsPage } from '../pages/SecretsPage'

export const secretsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/secrets',
  component: SecretsPage,
})
