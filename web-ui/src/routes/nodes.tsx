import { createRoute } from '@tanstack/react-router'
import { rootRoute } from './root'
import { NodeKeysPage } from '../pages/NodeKeysPage'

export const nodeKeysRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/keyholders',
  component: NodeKeysPage,
})
