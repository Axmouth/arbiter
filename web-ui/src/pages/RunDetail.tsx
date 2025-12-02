import type { JobRun } from "../backend-types/JobRun";
import { useJobs } from "../hooks/useJobs";

export function RunDetail({ run }: { run: JobRun }) {
  const { data: jobs } = useJobs();

  return (
    <div className="space-y-6">

      <div>
        <h3 className="text-sm font-semibold text-gray-700">Job</h3>
        <p className="mt-1">{jobs?.find(job => job.id === run.job_id)?.name ?? "<Unknown Job>"}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-700">State</h3>
        <p className="mt-1">{run.state}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-700">Command</h3>
        <pre className="bg-gray-100 p-3 rounded mt-1 text-sm whitespace-pre-wrap">
          {run.command}
        </pre>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-700">Started At</h3>
        <p className="mt-1">{format(run.started_at)}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-700">Finished At</h3>
        <p className="mt-1">{format(run.finished_at)}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-700">Scheduled For</h3>
        <p className="mt-1">{format(run.scheduled_for)}</p>
      </div>

      {run.exit_code != null && (
        <div>
          <h3 className="text-sm font-semibold text-gray-700">Exit Code</h3>
          <p className="mt-1">{run.exit_code}</p>
        </div>
      )}

    </div>
  );
}

function format(t?: string | null) {
  if (!t) return "—";
  return new Date(t).toLocaleString();
}
