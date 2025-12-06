import { createRoute } from '@tanstack/react-router'
import { rootRoute } from './root'
import { HomePage } from '../pages/HomePage'

export const homeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: HomePage,
})
