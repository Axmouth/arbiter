import { useWorkers } from "../hooks/useWorkers";

export function WorkersPage() {
  const { data: workers, isLoading, error } = useWorkers();

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-semibold">Workers</h2>

      {isLoading && <p>Loading…</p>}
      {error && <p className="text-red-600">{String(error)}</p>}

      {workers && (
        <div className="rounded-lg shadow border border-gray-200 overflow-hidden bg-white">
          <table className="w-full text-left">
            <thead className="bg-gray-50 text-gray-700">
              <tr>
                <th className="px-4 py-2 font-semibold">Display Name</th>
                <th className="px-4 py-2 font-semibold">Hostname</th>
                <th className="px-4 py-2 font-semibold">Last Seen</th>
                <th className="px-4 py-2 font-semibold">Capacity</th>
                <th className="px-4 py-2 font-semibold">Status</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-gray-200">
              {workers.map((w) => (
                <tr key={w.id} className="hover:bg-gray-50">
                  <td className="px-4 py-2">{w.displayName}</td>
                  <td className="px-4 py-2">{w.hostname}</td>
                  <td className="px-4 py-2">{formatTime(w.lastSeen)}</td>
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
  );
}

function formatTime(t: string) {
  return new Date(t).toLocaleString();
}

function WorkerStatus({ lastSeen }: { lastSeen: string }) {
  // TODO: Can it be better?
  // eslint-disable-next-line react-hooks/purity
  const delta = Date.now() - new Date(lastSeen).getTime();
  const alive = delta < 20_000; // 10 seconds threshold

  return (
    <span
      className={
        "px-2 py-1 rounded text-xs " +
        (alive
          ? "bg-green-100 text-green-700"
          : "bg-red-100 text-red-700")
      }
    >
      {alive ? "Online" : "Offline"}
    </span>
  );
}
