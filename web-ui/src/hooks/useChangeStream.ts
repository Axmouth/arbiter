import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'

/**
 * Subscribe to a server change-stream (SSE) and invalidate queries under `invalidateKey` on
 * each ping, so a page refetches on change instead of polling on a fixed timer. The browser
 * EventSource sends the session cookie, so no extra auth wiring is needed. Reusable across
 * resources (pass the resource's query-key prefix, e.g. 'runs').
 */
export function useChangeStream(path: string, invalidateKey: string) {
  const qc = useQueryClient()
  useEffect(() => {
    const es = new EventSource(path, { withCredentials: true })
    es.addEventListener('change', () => {
      qc.invalidateQueries({ queryKey: [invalidateKey] })
    })
    return () => es.close()
  }, [path, invalidateKey, qc])
}
