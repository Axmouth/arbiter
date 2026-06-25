import { useQuery } from '@tanstack/react-query'
import type { JobRun } from '../backend-types/JobRun'
import { fetchRuns } from '../api/runs'
import type { ListRunsQuery } from '../backend-types'

export function useJobRunsForJob(
  jobId: string | null,
  query: ListRunsQuery = {}
) {
  return useQuery<JobRun[]>({
    queryKey: ['runs', 'job', jobId, query],
    enabled: !!jobId,
    queryFn: () => fetchRuns({ ...query, byJobId: jobId ?? undefined }),
    // Liveness comes from the runs change-stream (useChangeStream), not a fixed poll.
  })
}
