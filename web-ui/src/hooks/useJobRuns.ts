import { useQuery } from '@tanstack/react-query'
import type { JobRun } from '../backend-types/JobRun'
import { fetchRuns } from '../api/runs'

export function useJobRunsForJob(jobId: string | null) {
  return useQuery<JobRun[]>({
    queryKey: ['runs', 'job', jobId],
    enabled: !!jobId,
    queryFn: () => fetchRuns({ byJobId: jobId ?? undefined }),
    refetchInterval: 15000, // auto-refresh history every 15s
  })
}
