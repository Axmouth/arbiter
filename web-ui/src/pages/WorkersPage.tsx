import { useEffect, useState } from 'react'
import { useWorkers } from '../hooks/useWorkers'
import { useChangeStream } from '../hooks/useChangeStream'

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
        <div
          className="
            rounded-lg border border-(--border-color) overflow-hidden
            border border-(--border-color)
            bg-(--bg-surface-alt)
          "
        >
          <table className="w-full text-left">
            <thead
              className="
                bg-(--bg-header)
                border-b border-(--border-subtle)
                text-(--text-primary)
              "
            >
              <tr>
                <th className="px-3 py-1.5 font-semibold">Display Name</th>
                <th className="px-3 py-1.5 font-semibold">Hostname</th>
                <th className="px-3 py-1.5 font-semibold">Last Seen</th>
                <th className="px-3 py-1.5 font-semibold">Restart Count</th>
                <th className="px-3 py-1.5 font-semibold">Version</th>
                <th className="px-3 py-1.5 font-semibold">Capacity</th>
                <th className="px-3 py-1.5 font-semibold">Status</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-(--border-subtle)">
              {workers.map((w) => (
                <tr key={w.id} className="hover:bg-(--bg-row-hover)">
                  <td className="px-3 py-1.5">{w.displayName}</td>
                  <td className="px-3 py-1.5">{w.hostname}</td>
                  <td className="px-3 py-1.5">{formatTime(w.lastSeen)}</td>
                  <td className="px-3 py-1.5">{w.restartCount}</td>
                  <td className="px-3 py-1.5">{w.version}</td>
                  <td className="px-3 py-1.5">{w.capacity}</td>
                  <td className="px-3 py-1.5">
                    <WorkerStatus lastSeen={w.lastSeen} now={now} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
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
