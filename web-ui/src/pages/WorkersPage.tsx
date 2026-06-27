import { useEffect, useState } from 'react'
import { useWorkers } from '../hooks/useWorkers'
import { useChangeStream } from '../hooks/useChangeStream'
import { Table, THead, Th, TBody, Tr, Td } from '../components/Table'

export function WorkersPage() {
  const { data: workers, isLoading, error } = useWorkers()
  // Live worker set (register/reclaim + presence aging) via the workers change-stream.
  useChangeStream('/api/v1/workers/stream', 'workers')
  // A ticking clock so "online/offline" stays current without reading Date.now() during
  // render (which is impure). Refreshes every 5s.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 5000)
    return () => clearInterval(id)
  }, [])

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold text-(--text-primary)">Workers</h2>

      {isLoading && <p className="text-(--text-muted)">Loading…</p>}

      {error && <p className="text-(--text-danger)">{String(error)}</p>}

      {workers && (
        <Table>
          <THead>
            <Th>Display Name</Th>
            <Th>Hostname</Th>
            <Th>Last Seen</Th>
            <Th>Restart Count</Th>
            <Th>Version</Th>
            <Th>Capacity</Th>
            <Th>Status</Th>
          </THead>
          <TBody>
            {workers.map((w) => (
              <Tr key={w.id}>
                <Td>{w.displayName}</Td>
                <Td>{w.hostname}</Td>
                <Td>{formatTime(w.lastSeen)}</Td>
                <Td>{w.restartCount}</Td>
                <Td>{w.version}</Td>
                <Td>{w.capacity}</Td>
                <Td>
                  <WorkerStatus lastSeen={w.lastSeen} now={now} />
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
      )}
    </div>
  )
}

function formatTime(t: string) {
  return new Date(t).toLocaleString()
}

function WorkerStatus({ lastSeen, now }: { lastSeen: string; now: number }) {
  // `now` is a ticking value from the parent, so this stays a pure render.
  const delta = now - new Date(lastSeen).getTime()
  const alive = delta < 20_000 // 20 seconds threshold

  return (
    <span
      className={`px-2 py-1 rounded text-xs ${
        alive
          ? 'bg-(--bg-success) text-(--text-success)'
          : 'bg-(--bg-error) text-(--text-error)'
      }`}
    >
      {alive ? 'Online' : 'Offline'}
    </span>
  )
}
