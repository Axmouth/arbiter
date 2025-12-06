import type { JobRun } from "../backend-types/JobRun";
import { formatTime } from "../utils/time";

export function JobRunHistory({
  runs,
  onSelect,
}: {
  runs: JobRun[];
  onSelect?: (run: JobRun) => void;
}) {
  if (runs.length === 0) {
    return (
      <p className="text-sm text-gray-500">
        No runs recorded yet.
      </p>
    );
  }

  return (
    <div className="rounded border border-gray-200 overflow-hidden">
      <table className="w-full text-left text-sm">
        <thead className="bg-gray-50">
          <tr>
            <th className="px-3 py-2 font-semibold">State</th>
            <th className="px-3 py-2 font-semibold">Scheduled</th>
            <th className="px-3 py-2 font-semibold">Started</th>
            <th className="px-3 py-2 font-semibold">Finished</th>
          </tr>
        </thead>

        <tbody className="divide-y divide-gray-200">
          {runs.map((run) => (
            <tr
              key={run.id}
              className={
                "hover:bg-gray-50 " +
                (onSelect ? "cursor-pointer" : "")
              }
              onClick={() => onSelect?.(run)}
            >
              <td className="px-3 py-2">
                <span
                  className={
                    "px-2 py-1 rounded text-xs " +
                    (run.state === "succeeded"
                      ? "bg-green-100 text-green-700"
                      : run.state === "failed"
                      ? "bg-red-100 text-red-700"
                      : "bg-gray-100 text-gray-700")
                  }
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
  );
}
