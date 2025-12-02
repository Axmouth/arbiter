import { useState } from "react";
import { useRuns } from "../hooks/useRuns";
import { SlideOver } from "../components/SlideOver";
import type { JobRun } from "../backend-types/JobRun";
import { RunDetail } from "./RunDetail";
import { useJobs } from "../hooks/useJobs";

export function RunsPage() {
  const { data: runs, isLoading, error } = useRuns();
  const { data: jobs, error: jobsError } = useJobs();
  const [selected, setSelected] = useState<JobRun | null>(null);

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-semibold">Recent Runs</h2>

      {isLoading && <p>Loading…</p>}
      {error && <p className="text-red-600">{String(error)}</p>}
      {jobsError && <p className="text-red-600">{String(jobsError)}</p>}

      {runs && (
        <div className="rounded-lg shadow border border-gray-200 overflow-hidden bg-white">
          <table className="w-full text-left">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-4 py-2 font-semibold">Job</th>
                <th className="px-4 py-2 font-semibold">State</th>
                <th className="px-4 py-2 font-semibold">Started</th>
                <th className="px-4 py-2 font-semibold">Finished</th>
                <th className="px-4 py-2 font-semibold">Scheduled for</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-gray-200">
              {runs.map(run => (
                <tr
                  key={run.id}
                  className="hover:bg-gray-50 cursor-pointer"
                  onClick={() => setSelected(run)}
                >
                  <td className="px-4 py-2">{jobs?.find(job => job.id === run.job_id)?.name ?? "<Unknown Job>"}</td>
                  <td className="px-4 py-2">
                    <RunStateBadge state={run.state} />
                  </td>
                  <td className="px-4 py-2">{formatTime(run.started_at)}</td>
                  <td className="px-4 py-2">{formatTime(run.finished_at)}</td>
                  <td className="px-4 py-2">{formatTime(run.scheduled_for)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <SlideOver
        open={!!selected}
        onClose={() => setSelected(null)}
        title={`Run ${selected?.id}`}
      >
        {selected && <RunDetail run={selected} />}
      </SlideOver>
    </div>
  );
}

function formatTime(t?: string | null) {
  if (!t) return "—";
  return new Date(t).toLocaleString();
}

function RunStateBadge({ state }: { state: JobRun["state"] }) {
  const style =
    state === "succeeded"
      ? "bg-green-100 text-green-700"
      : state === "failed"
      ? "bg-red-100 text-red-700"
      : state === "running"
      ? "bg-blue-100 text-blue-700"
      : state === "cancelled"
      ? "bg-gray-300 text-gray-700"
      : "bg-yellow-100 text-yellow-700";

  return (
    <span className={`px-2 py-1 rounded text-xs ${style}`}>
      {state}
    </span>
  );
}
