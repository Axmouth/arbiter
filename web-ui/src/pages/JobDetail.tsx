import type { JobSpec } from '../backend-types/JobSpec'
import { JobRunHistory } from '../components/JobRunHistory'
import { useJobRunsForJob } from '../hooks/useJobRuns'
import { misfirePolicyLabel } from '../utils/misfire'
import cronstrue from 'cronstrue'

export type JobDetailsViewProps = {
  job: JobSpec
  onEdit: () => void
  onRunNow: () => void
  onDelete: () => void
  onToggleEnabled: () => void
  onComplete: (job: JobSpec | null) => void
}

export function JobDetailsView({
  job,
  onEdit,
  onRunNow,
  onDelete,
  onToggleEnabled,
}: JobDetailsViewProps) {
  const { data: runs, isLoading: runsLoading } = useJobRunsForJob(job.id, {
    limit: 50,
  })

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">Name</h3>
        <p className="mt-1">{job.name}</p>
      </div>

      <div>
        <h3 className="text-sm font-semibold">Schedule</h3>
        <p className="mt-1 text-(--text-primary)">{job.scheduleCron ?? '—'}</p>

        {job.scheduleCron && (
          <p className="text-sm text-(--text-muted)">
            {cronstrue.toString(job.scheduleCron)}
          </p>
        )}
      </div>

      <div>
        <h3 className="text-sm font-semibold">Command</h3>
        <pre className="bg-(--bg-code) text-(--text-code) p-3 rounded mt-1 text-sm whitespace-pre-wrap">
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
          className="
            px-4 py-2 rounded
            bg-(--bg-btn-primary)
            text-(--text-inverse)
            hover:bg-(--bg-btn-primary-hover)
          "
        >
          Edit
        </button>

        <button
          onClick={onToggleEnabled}
          className={`
            px-4 py-2 rounded text-(--text-inverse)
            ${
              job.enabled
                ? 'bg-(--bg-btn-warning) hover:bg-(--bg-btn-warning-hover)'
                : 'bg-(--bg-btn-positive) hover:bg-(--bg-btn-positive-hover)'
            }
          `}
        >
          {job.enabled ? 'Disable' : 'Enable'}
        </button>

        <button
          onClick={onRunNow}
          className="
            px-4 py-2 rounded
            bg-(--bg-btn-positive)
            text-(--text-inverse)
            hover:bg-(--bg-btn-positive-hover)
          "
        >
          Run Now
        </button>

        <button
          onClick={onDelete}
          className="
            px-4 py-2 rounded
            bg-(--bg-btn-danger)
            text-(--text-inverse)
            hover:bg-(--bg-btn-danger-hover)
          "
        >
          Delete
        </button>
      </div>

      <div className="pt-4 border-t">
        <h3 className="text-sm font-semibold mb-2">Recent Runs</h3>

        {runsLoading && <p className="text-(--text-muted)">Loading…</p>}

        {runs && (
          <JobRunHistory
            runs={runs}
            onSelect={(run) => {
              // later: open run detail slide-over
              console.log('Selected run:', run.id)
            }}
          />
        )}
      </div>
    </div>
  )
}
