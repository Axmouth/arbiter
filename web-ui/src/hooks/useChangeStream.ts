import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'

/**
 * Subscribe to a server change-stream (SSE) and invalidate queries under `invalidateKey` when
 * it pings, so a page refetches on change instead of polling on a fixed timer. The browser
 * EventSource sends the session cookie, so no extra auth wiring is needed. Reusable across
 * resources (pass the resource's query-key prefix, e.g. 'runs').
 *
 * Invalidation is debounced: a burst of pings (the connect ping plus several state
 * transitions arriving together) collapses into one refetch after `debounceMs` of quiet, so
 * the view does not flicker through several rapid refetches.
 */
export function useChangeStream(path: string, invalidateKey: string, debounceMs = 300) {
  const qc = useQueryClient()
  useEffect(() => {
    const es = new EventSource(path, { withCredentials: true })
    let timer: ReturnType<typeof setTimeout> | undefined
    es.addEventListener('change', () => {
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        qc.invalidateQueries({ queryKey: [invalidateKey] })
      }, debounceMs)
    })
    return () => {
      if (timer) clearTimeout(timer)
      es.close()
    }
  }, [path, invalidateKey, qc, debounceMs])
}
