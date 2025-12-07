import type { JobRun } from '../backend-types/JobRun'
import { formatTime } from '../utils/time'

export function JobRunHistory({
  runs,
  onSelect,
}: {
  runs: JobRun[]
  onSelect?: (run: JobRun) => void
}) {
  if (runs.length === 0) {
    return <p className="text-sm text-(--text-muted)">No runs recorded yet.</p>
  }

  return (
    <div className="rounded border border-(--border-color) overflow-hidden bg-(--bg-surface-alt) text-(--text-secondary)">
      <table className="w-full text-left text-sm">
        <thead className="bg-(--bg-header) border-b border-(--border-subtle)">
          <tr>
            <th className="px-3 py-2 font-semibold">State</th>
            <th className="px-3 py-2 font-semibold">Scheduled</th>
            <th className="px-3 py-2 font-semibold">Started</th>
            <th className="px-3 py-2 font-semibold">Finished</th>
          </tr>
        </thead>

        <tbody className="divide-y divide-(--border-subtle)">
          {runs.map((run) => (
            <tr
              key={run.id}
              className={`hover:bg-(--bg-hover) ${onSelect ? 'cursor-pointer' : ''}`}
              onClick={() => onSelect?.(run)}
            >
              <td className="px-3 py-2">
                <span
                  className={`px-2 py-1 rounded text-xs ${
                    run.state === 'succeeded'
                      ? 'bg-(--bg-success) text-(--text-success)'
                      : run.state === 'failed'
                        ? 'bg-(--bg-error) text-(--text-error)'
                        : 'bg-(--bg-neutral) text-(--text-neutral)'
                  }`}
                >
                  {run.state}
                </span>
              </td>
              <td className="px-3 py-2">{formatTime(run.scheduledFor)}</td>
              <td className="px-3 py-2">{formatTime(run.startedAt)}</td>
              <td className="px-3 py-2">{formatTime(run.finishedAt)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
