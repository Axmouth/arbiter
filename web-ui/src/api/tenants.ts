import type { CreateTenantRequest, Tenant } from '../backend-types'
import { api } from './client'

export function fetchTenants(): Promise<Tenant[]> {
  return api<Tenant[]>('/tenants')
}

export function createTenant(req: CreateTenantRequest): Promise<Tenant> {
  return api<Tenant>('/tenants', {
    method: 'POST',
    body: JSON.stringify(req),
  })
}
