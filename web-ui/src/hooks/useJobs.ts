import { useQuery } from '@tanstack/react-query'
import { fetchJobs } from '../api/jobs'

export function useJobs() {
  // Liveness comes from the jobs change-stream (useChangeStream on JobsPage), not a poll.
  return useQuery({
    queryKey: ['jobs'],
    queryFn: fetchJobs,
  })
}
