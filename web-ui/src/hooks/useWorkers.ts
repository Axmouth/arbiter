import { useQuery } from '@tanstack/react-query'
import type { WorkerRecord } from '../backend-types/WorkerRecord'
import { fetchWorkers } from '../api/workers'

export function useWorkers() {
  // Liveness comes from the workers change-stream (useChangeStream on WorkersPage), which
  // pings on register/reclaim plus a short backstop tick for presence (offline) aging.
  return useQuery<WorkerRecord[]>({
    queryKey: ['workers'],
    queryFn: fetchWorkers,
  })
}
