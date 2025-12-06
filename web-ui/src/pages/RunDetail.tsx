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
        <h3 className="text-sm font-semibold text-gray-700">Job</h3>
        <p className="mt-1">
          {jobs?.find((job) => job.id === run.jobId)?.name ?? '<Unknown Job>'}
        </p>
      </div>

      {/* Run State */}
      <div>
        <h3 className="text-sm font-semibold text-gray-700">State</h3>
        <p className="mt-1">{run.state}</p>
      </div>

      {/* Command */}
      <div>
        <h3 className="text-sm font-semibold text-gray-700">Command</h3>
        <pre className="bg-gray-100 p-3 rounded mt-1 text-sm whitespace-pre-wrap">
          {run.snapshot.meta.type === 'shell' ? run.snapshot.meta.command : ''}
        </pre>
      </div>

      {/* Started At*/}
      <div>
        <h3 className="text-sm font-semibold text-gray-700">Started At</h3>
        <p className="mt-1">{format(run.startedAt)}</p>
      </div>

      {/* Finished At */}
      <div>
        <h3 className="text-sm font-semibold text-gray-700">Finished At</h3>
        <p className="mt-1">{format(run.finishedAt)}</p>
      </div>

      {/* Scheduled For */}
      <div>
        <h3 className="text-sm font-semibold text-gray-700">Scheduled For</h3>
        <p className="mt-1">{format(run.scheduledFor)}</p>
      </div>

      {/* Exit Code */}
      {run.exitCode != null && (
        <div>
          <h3 className="text-sm font-semibold text-gray-700">Exit Code</h3>
          <p className="mt-1">{run.exitCode}</p>
        </div>
      )}

      {/* Output */}
      {isFinished() && (
        <div>
          <h3 className="text-sm font-semibold text-gray-700">Output</h3>
          <div className="mt-1 bg-gray-50 border rounded p-3 max-h-64 overflow-auto text-sm whitespace-pre-wrap">
            {run.output && run.output.trim() !== '' && isFinished() ? (
              <pre>{run.output}</pre>
            ) : (
              <span className="text-gray-400 italic">&lt;Empty&gt;</span>
            )}
          </div>
        </div>
      )}

      {/* Error Output - only render if present */}
      {run.errorOutput && run.errorOutput.trim() !== '' && isFinished() && (
        <div>
          <h3 className="text-sm font-semibold text-gray-700">Error Output</h3>
          <div className="mt-1 bg-red-50 border border-red-200 rounded p-3 max-h-64 overflow-auto text-sm whitespace-pre-wrap text-red-800">
            <pre>{run.errorOutput}</pre>
          </div>
        </div>
      )}

      <div className="pt-6 flex gap-3">
        {isPending() ? (
          <button
            onClick={() => cancelMutation.mutate()}
            className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700"
          >
            Cancel Run
          </button>
        ) : (
          <button
            onClick={() => rerunMutation.mutate()}
            className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700"
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
