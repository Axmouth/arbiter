import { useEffect, useState } from 'react'
import type { JobRun } from '../backend-types/JobRun'

/**
 * Stream a single run's state and captured output over SSE (`/runs/{id}/stream`). The server
 * pushes a fresh JobRun on each change and closes the stream once the run is terminal. The
 * browser EventSource sends the session cookie, so no extra auth wiring is needed. Returns
 * the latest streamed run, or null until the first event arrives.
 */
export function useRunStream(runId: string | null): JobRun | null {
  const [run, setRun] = useState<JobRun | null>(null)

  useEffect(() => {
    if (!runId) return
    const es = new EventSource(`/api/v1/runs/${runId}/stream`, { withCredentials: true })
    es.onmessage = (e) => setRun(JSON.parse(e.data) as JobRun)
    es.onerror = () => es.close()
    return () => es.close()
  }, [runId])

  return run
}
