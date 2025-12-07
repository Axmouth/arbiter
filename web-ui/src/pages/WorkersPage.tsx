import { useWorkers } from '../hooks/useWorkers'

export function WorkersPage() {
  const { data: workers, isLoading, error } = useWorkers()

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-semibold text-(--text-primary)">Workers</h2>

      {isLoading && <p className="text-(--text-muted)">Loading…</p>}

      {error && <p className="text-(--text-danger)">{String(error)}</p>}

      {workers && (
        <div
          className="
            rounded-lg shadow overflow-hidden
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
                <th className="px-4 py-2 font-semibold">Display Name</th>
                <th className="px-4 py-2 font-semibold">Hostname</th>
                <th className="px-4 py-2 font-semibold">Last Seen</th>
                <th className="px-4 py-2 font-semibold">Restart Count</th>
                <th className="px-4 py-2 font-semibold">Version</th>
                <th className="px-4 py-2 font-semibold">Capacity</th>
                <th className="px-4 py-2 font-semibold">Status</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-(--border-subtle)">
              {workers.map((w) => (
                <tr key={w.id} className="hover:bg-(--bg-row-hover)">
                  <td className="px-4 py-2">{w.displayName}</td>
                  <td className="px-4 py-2">{w.hostname}</td>
                  <td className="px-4 py-2">{formatTime(w.lastSeen)}</td>
                  <td className="px-4 py-2">{w.restartCount}</td>
                  <td className="px-4 py-2">{w.version}</td>
                  <td className="px-4 py-2">{w.capacity}</td>
                  <td className="px-4 py-2">
                    <WorkerStatus lastSeen={w.lastSeen} />
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

function WorkerStatus({ lastSeen }: { lastSeen: string }) {
  // TODO: Can it be better?
  // eslint-disable-next-line react-hooks/purity
  const delta = Date.now() - new Date(lastSeen).getTime()
  const alive = delta < 20_000 // 10 seconds threshold

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
