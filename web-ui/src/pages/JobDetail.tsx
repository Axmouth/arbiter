import { useQuery } from '@tanstack/react-query'
import type { JobSpec } from '../backend-types/JobSpec'
import type { RunnerConfig } from '../backend-types'
import { JobRunHistory } from '../components/JobRunHistory'
import { useJobRunsForJob } from '../hooks/useJobRuns'
import { useChangeStream } from '../hooks/useChangeStream'
import { fetchJobEnv } from '../api/jobs'
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
  // Live run history via the runs change-stream instead of a fixed poll.
  useChangeStream('/api/v1/runs/stream', 'runs')
  const { data: env } = useQuery({
    queryKey: ['job-env', job.id],
    queryFn: () => fetchJobEnv(job.id),
  })
  const envEntries = Object.entries(env ?? {})

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
        <h3 className="text-sm font-semibold">Runner</h3>
        <RunnerSummary cfg={job.runnerCfg} />
      </div>

      {envEntries.length > 0 && (
        <div>
          <h3 className="text-sm font-semibold">Environment</h3>
          <div className="mt-1 space-y-1 font-mono text-sm">
            {envEntries.map(([k, v]) => (
              <div key={k} className="flex gap-2">
                <span className="text-(--text-secondary)">{k}</span>
                <span className="text-(--text-muted)">=</span>
                <span className="text-(--text-primary)">{v}</span>
              </div>
            ))}
          </div>
        </div>
      )}

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

const RUNNER_TYPE_LABEL: Record<RunnerConfig['type'], string> = {
  shell: 'Shell',
  http: 'HTTP',
  pgSql: 'PostgreSQL',
  mySql: 'MySQL',
  python: 'Python',
  node: 'Node',
}

function RunnerSummary({ cfg }: { cfg: RunnerConfig }) {
  return (
    <div className="mt-1 space-y-2">
      <p className="text-sm text-(--text-muted)">{RUNNER_TYPE_LABEL[cfg.type]}</p>
      {cfg.type === 'shell' && <Code>{cfg.command}</Code>}
      {cfg.type === 'http' && (
        <Code>
          {cfg.method} {cfg.url}
        </Code>
      )}
      {(cfg.type === 'pgSql' || cfg.type === 'mySql') && <Code>{cfg.query}</Code>}
      {cfg.type === 'python' && (
        <Code>
          {cfg.module}.{cfg.className}
        </Code>
      )}
      {cfg.type === 'node' && (
        <Code>
          {cfg.module} → {cfg.functionName}
        </Code>
      )}
    </div>
  )
}

function Code({ children }: { children: React.ReactNode }) {
  return (
    <pre className="bg-(--bg-code) text-(--text-code) p-3 rounded text-sm whitespace-pre-wrap">
      {children}
    </pre>
  )
}
