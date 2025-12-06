import type { JobSpec } from "../backend-types/JobSpec";
import { JobRunHistory } from "../components/JobRunHistory";
import { useJobRunsForJob } from "../hooks/useJobRuns";
import { misfirePolicyLabel } from "../utils/misfire";
import cronstrue from "cronstrue";

export type JobDetailsViewProps = {
  job: JobSpec;
  onEdit: () => void;
  onRunNow: () => void;
  onDelete: () => void;
  onToggleEnabled: () => void;
  onComplete: (job: JobSpec | null) => void;
};

export function JobDetailsView({ job, onEdit, onRunNow, onDelete, onToggleEnabled }: JobDetailsViewProps) {
  const { data: runs, isLoading: runsLoading } = useJobRunsForJob(job.id);

  return (
    <div className="space-y-6">

      <div>
        <h3 className="text-sm font-semibold">Name</h3>
        <p className="mt-1">{job.name}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold">Schedule</h3>
        <p className="mt-1 text-gray-900">{job.scheduleCron ?? "—"}</p>
        
        {job.scheduleCron && (
          <p className="text-sm text-gray-500">
            {cronstrue.toString(job.scheduleCron)}
          </p>
        )}
      </div>

      <div>
        <h3 className="text-sm font-semibold">Command</h3>
        <pre className="bg-gray-100 p-3 rounded mt-1 text-sm whitespace-pre-wrap">
          {job?.runnerCfg.type === 'shell' ? job?.runnerCfg.command : ''}
        </pre>
      </div>

      <div>
        <h3 className="text-sm font-semibold">Concurrency</h3>
        <p className="mt-1">{job.maxConcurrency}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold">Misfire Policy</h3>
        <p className="mt-1">{misfirePolicyLabel(job.misfirePolicy)}</p>
      </div>

      {/* Action buttons */}
      <div className="pt-6 flex gap-3">
        <button
          onClick={onEdit}
          className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700"
        >
          Edit
        </button>

        <button
          onClick={onToggleEnabled}
          className={
            job.enabled
              ? "px-4 py-2 bg-yellow-500 text-white rounded hover:bg-yellow-600"
              : "px-4 py-2 bg-purple-600 text-white rounded hover:bg-purple-700"
          }
        >
          {job.enabled ? "Disable" : "Enable"}
        </button>

        <button
          onClick={onRunNow}
          className="px-4 py-2 bg-green-600 text-white rounded hover:bg-green-700"
        >
          Run Now
        </button>

        <button
          onClick={onDelete}
          className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700"
        >
          Delete
        </button>
      </div>

      
      <div className="pt-4 border-t">
        <h3 className="text-sm font-semibold mb-2">Recent Runs</h3>

        {runsLoading && <p className="text-gray-500">Loading…</p>}

        {runs && (
          <JobRunHistory
            runs={runs}
            onSelect={(run) => {
              // later: open run detail slide-over
              console.log("Selected run:", run.id);
            }}
          />
        )}
      </div>
    </div>
  );
}
