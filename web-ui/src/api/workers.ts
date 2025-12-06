import type { WorkerRecord } from '../backend-types'
import { api } from './client'

export function fetchWorkers(): Promise<WorkerRecord[]> {
  return api<WorkerRecord[]>('/workers', {
    method: 'GET',
    headers: { 'Content-Type': 'application/json' },
  })
}
