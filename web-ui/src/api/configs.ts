import type {
  CreateDbConfigRequest,
  SharedDbConfig,
  UpdateDbConfigRequest,
} from '../backend-types'
import { api } from './client'

export function fetchDbConfigs(): Promise<SharedDbConfig[]> {
  return api<SharedDbConfig[]>('/db-configs')
}

export function createDbConfig(
  req: CreateDbConfigRequest
): Promise<SharedDbConfig> {
  return api<SharedDbConfig>('/db-configs', {
    method: 'POST',
    body: JSON.stringify(req),
  })
}

export function updateDbConfig(
  id: string,
  req: UpdateDbConfigRequest
): Promise<SharedDbConfig> {
  return api<SharedDbConfig>(`/db-configs/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(req),
  })
}

export function deleteDbConfig(id: string): Promise<void> {
  return api<void>(`/db-configs/${id}`, { method: 'DELETE' })
}
