import { createRoute } from '@tanstack/react-router'
import { rootRoute } from './root'
import { UsersPage } from '../pages/UsersPage'

export const usersRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/users',
  component: UsersPage,
})
