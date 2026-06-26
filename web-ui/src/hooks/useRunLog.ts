import { useCallback, useEffect, useState } from 'react'
import type { JobRun } from '../backend-types/JobRun'
import type { LogChunk } from '../backend-types'
import { fetchRunLogs } from '../api/runs'

const TERMINAL = ['succeeded', 'failed', 'cancelled']
const PAGE = 500

/**
 * Drive a single run's live view. Loads the tail of the log up front (works for both running
 * and finished runs), and for a still-running run opens the multiplexed SSE
 * (`/runs/{id}/stream?after=<cursor>`) to receive `state` updates and append-only `log` chunk
 * deltas without re-fetching what the page already has. `loadEarlier` pages backward.
 *
 * Mount this per run (key the component by run id) so its state resets cleanly on selection
 * change, which also keeps the effect free of synchronous resets.
 */
export function useRunLog(initial: JobRun): {
  run: JobRun
  chunks: LogChunk[]
  loadEarlier: () => void
  loadingEarlier: boolean
  hasEarlier: boolean
} {
  const [run, setRun] = useState<JobRun>(initial)
  const [chunks, setChunks] = useState<LogChunk[]>([])
  const [loadingEarlier, setLoadingEarlier] = useState(false)

  useEffect(() => {
    let es: EventSource | null = null
    let cancelled = false
    void (async () => {
      const page = await fetchRunLogs(initial.id, { limit: PAGE }).catch(() => null)
      if (cancelled || !page) return
      setChunks(page.chunks)
      if (TERMINAL.includes(initial.state)) return
      // Still running: follow live from the last loaded seq so chunks are not re-sent.
      const after = page.size.maxSeq
      const qs = after != null ? `?after=${after}` : ''
      es = new EventSource(`/api/v1/runs/${initial.id}/stream${qs}`, { withCredentials: true })
      es.addEventListener('state', (e) => setRun(JSON.parse((e as MessageEvent).data) as JobRun))
      es.addEventListener('log', (e) => {
        const batch = JSON.parse((e as MessageEvent).data) as LogChunk[]
        setChunks((prev) => {
          const maxSeq = prev.length ? prev[prev.length - 1].seq : -1n
          const fresh = batch.filter((c) => c.seq > maxSeq)
          return fresh.length ? [...prev, ...fresh] : prev
        })
      })
      es.onerror = () => es?.close()
    })()
    return () => {
      cancelled = true
      es?.close()
    }
  }, [initial.id, initial.state])

  const loadEarlier = useCallback(() => {
    if (chunks.length === 0) return
    setLoadingEarlier(true)
    fetchRunLogs(initial.id, { before: chunks[0].seq, limit: PAGE })
      .then((page) => setChunks((prev) => [...page.chunks, ...prev]))
      .catch(() => {})
      .finally(() => setLoadingEarlier(false))
  }, [chunks, initial.id])

  const hasEarlier = chunks.length > 0 && chunks[0].seq > 0n

  return { run, chunks, loadEarlier, loadingEarlier, hasEarlier }
}
