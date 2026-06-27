import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { fetchRuns } from '../api/runs'
import type { ListRunsQuery } from '../backend-types'

export function useRuns(query: ListRunsQuery = {}, pollMs: number = 2000) {
  return useQuery({
    queryKey: ['runs', query],
    queryFn: () => fetchRuns(query),
    refetchInterval: pollMs > 0 ? pollMs : false, // manual mode => off
    // Keep the prior rows on screen during a refetch or a key change (e.g. load-more grows
    // the limit), so the table never blanks or shifts.
    placeholderData: keepPreviousData,
  })
}
