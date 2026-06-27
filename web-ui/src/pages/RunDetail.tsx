import { useMutation, useQueryClient } from '@tanstack/react-query'
import type { JobRun } from '../backend-types/JobRun'
import { useJobs } from '../hooks/useJobs'
import { useRunLog } from '../hooks/useRunLog'
import { RunLogView } from '../components/RunLogView'
import { Button } from '../components/Button'
import { cancelRun } from '../api/runs'
import { runJobNow } from '../api/jobs'

export function RunDetail({ run: runProp }: { run: JobRun }) {
  const { data: jobs } = useJobs()
  const qc = useQueryClient()
  // Live: load the log tail and (for a running run) follow state + output over SSE. Mounted
  // per run id by the caller, so state resets cleanly on selection change.
  const { run, chunks, loadEarlier, loadingEarlier, hasEarlier } = useRunLog(runProp)
  const live = !['succeeded', 'failed', 'cancelled'].includes(run.state)

  const cancelMutation = useMutation({
    mutationFn: () => cancelRun(run.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['runs'] })
    },
  })

  const rerunMutation = useMutation({
    mutationFn: () => runJobNow(run.jobId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['runs'] })
    },
  })

  const isPending = () => run.state === 'queued'

  return (
    <div className="space-y-6">
      {/* Job Name */}
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">Job</h3>
        <p className="mt-1 text-(--text-primary)">
          {jobs?.find((job) => job.id === run.jobId)?.name ?? '<Unknown Job>'}
        </p>
      </div>

      {/* Run State */}
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">State</h3>
        <p className="mt-1 text-(--text-primary)">{run.state}</p>
      </div>

      {/* Command */}
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">Command</h3>
        <pre
          className="
            bg-(--bg-code) text-(--text-code)
            p-3 rounded mt-1 text-sm whitespace-pre-wrap
          "
        >
          {run.snapshot?.meta.type === 'shell' ? run.snapshot.meta.command : ''}
        </pre>
      </div>

      {/* Started At*/}
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">
          Started At
        </h3>
        <p className="mt-1">{format(run.startedAt)}</p>
      </div>

      {/* Finished At */}
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">
          Finished At
        </h3>
        <p className="mt-1">{format(run.finishedAt)}</p>
      </div>

      {/* Scheduled For */}
      <div>
        <h3 className="text-sm font-semibold text-(--text-primary)">
          Scheduled For
        </h3>
        <p className="mt-1">{format(run.scheduledFor)}</p>
      </div>

      {/* Exit Code */}
      {run.exitCode != null && (
        <div>
          <h3 className="text-sm font-semibold text-(--text-primary)">
            Exit Code
          </h3>
          <p className="mt-1 text-(--text-primary)">{run.exitCode}</p>
        </div>
      )}

      {/* Output (live for a running run, paginated tail for a finished one) */}
      <RunLogView
        chunks={chunks}
        loadEarlier={loadEarlier}
        loadingEarlier={loadingEarlier}
        hasEarlier={hasEarlier}
        live={live}
      />

      <div className="pt-6 flex gap-3">
        {isPending() ? (
          <Button variant="danger" onClick={() => cancelMutation.mutate()}>
            Cancel Run
          </Button>
        ) : (
          // TODO: Disable if job is in running state?
          <Button variant="primary" onClick={() => rerunMutation.mutate()}>
            Re-run
          </Button>
        )}
      </div>
    </div>
  )
}

function format(t?: string | null) {
  if (!t) return '—'
  return new Date(t).toLocaleString()
}
