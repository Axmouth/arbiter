import { useMutation, useQueryClient } from '@tanstack/react-query'
import type { JobRun } from '../backend-types/JobRun'
import { useJobs } from '../hooks/useJobs'
import { cancelRun } from '../api/runs'
import { runJobNow } from '../api/jobs'

export function RunDetail({ run }: { run: JobRun }) {
  const { data: jobs } = useJobs()
  const qc = useQueryClient()

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
  const isFinished = () => ['succeeded', 'failed'].includes(run.state)

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

      {/* Output */}
      {isFinished() && (
        <div>
          <h3 className="text-sm font-semibold text-(--text-primary)">
            Output
          </h3>
          <div
            className="
                mt-1 p-3 max-h-64 overflow-auto text-sm whitespace-pre-wrap border rounded
                bg-(--bg-code) border-(--border-color)
              "
          >
            {run.output && run.output.trim() !== '' && isFinished() ? (
              <pre className="text-(--text-primary)">{run.output}</pre>
            ) : (
              <span className="text-(--text-muted) italic">&lt;Empty&gt;</span>
            )}
          </div>
        </div>
      )}

      {/* Error Output - only render if present */}
      {run.errorOutput && run.errorOutput.trim() !== '' && isFinished() && (
        <div>
          <h3 className="text-sm font-semibold text-(--text-primary)">
            Error Output
          </h3>
          <div
            className="
            mt-1 p-3 max-h-64 overflow-auto text-sm whitespace-pre-wrap rounded border
            bg-(--bg-error-soft) border-(--bg-error)
            text-(--text-error)
          "
          >
            <pre>{run.errorOutput}</pre>
          </div>
        </div>
      )}

      <div className="pt-6 flex gap-3">
        {isPending() ? (
          <button
            onClick={() => cancelMutation.mutate()}
            className="
              px-4 py-2 rounded
              bg-(--bg-btn-danger)
              text-(--text-inverse)
              hover:bg-(--bg-btn-danger-hover)
            "
          >
            Cancel Run
          </button>
        ) : (
          <button
            onClick={() => rerunMutation.mutate()}
            className="
              px-4 py-2 rounded
              bg-(--bg-btn-primary)
              text-(--text-inverse)
              hover:bg-(--bg-btn-primary-hover)
            "
          >
            Re-run
          </button>
        )}
      </div>
    </div>
  )
}

function format(t?: string | null) {
  if (!t) return '—'
  return new Date(t).toLocaleString()
}
