import type { ListRunsQuery, RunLogPage } from '../backend-types'
import type { JobRun } from '../backend-types/JobRun'
import { api } from './client'

/** Fetch a page of a run's log chunks. No cursor = the tail (most recent). `before` pages
 *  earlier, `after` catches up to live. */
export function fetchRunLogs(
  runId: string,
  opts: { after?: bigint; before?: bigint; limit?: number } = {}
): Promise<RunLogPage> {
  const query: Record<string, string> = {}
  if (opts.after != null) query.after = String(opts.after)
  if (opts.before != null) query.before = String(opts.before)
  if (opts.limit != null) query.limit = String(opts.limit)
  return api<RunLogPage>(`/runs/${runId}/logs`, { method: 'GET' }, query)
}

export function fetchRuns(query: ListRunsQuery): Promise<JobRun[]> {
  return api<JobRun[]>(
    '/runs',
    {
      method: 'GET',
      headers: { 'Content-Type': 'application/json' },
    },
    query as Record<string, string>
  )
}

export function cancelRun(id: string): Promise<void> {
  return api<void>(`/runs/${id}/cancel`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
  })
}
