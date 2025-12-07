import { Fragment, useMemo, useState } from 'react'
import { useRuns } from '../hooks/useRuns'
import { SlideOver } from '../components/SlideOver'
import type { JobRun } from '../backend-types/JobRun'
import { RunDetail } from './RunDetail'
import { useJobs } from '../hooks/useJobs'
import type { ListRunsQuery } from '../backend-types'
import { SearchableDropdown } from '../components/SearchableDropdown'
import { useWorkers } from '../hooks/useWorkers'
import {
  Listbox,
  ListboxButton,
  ListboxOption,
  ListboxOptions,
  Transition,
} from '@headlessui/react'

const POLL_OPTIONS = [
  { ms: 200, label: '0.2s', desc: 'Spammy' },
  { ms: 1000, label: '  1s', desc: 'Fast' },
  { ms: 2000, label: '  2s', desc: 'Normal' },
  { ms: 10000, label: ' 10s', desc: 'Chill' },
  { ms: 0, label: ' Off', desc: "I'll do it myself" },
]

export function PollSelect({
  pollMs,
  setPollMs,
}: {
  pollMs: number
  setPollMs: (n: number) => void
}) {
  return (
    <Listbox value={pollMs} onChange={setPollMs}>
      <div className="relative">
        <label className="block text-sm font-medium mb-1 text-(--text-primary)">
          Update Rate
        </label>

        {/* Compact button */}
        <ListboxButton
          className="
            w-20 text-left font-mono px-2 py-1 rounded border
            bg-(--bg-app) text-(--text-primary)
            border-(--border-color)
            hover:border-(--text-accent)
            focus:outline-none focus:ring-1 focus:ring-(--text-accent)
          "
        >
          {POLL_OPTIONS.find((p) => p.ms === pollMs)?.label ?? 'Select'}
        </ListboxButton>

        {/* Dropdown */}
        <Transition
          as={Fragment}
          enter="transition ease-out duration-100"
          enterFrom="opacity-0 scale-95"
          enterTo="opacity-100 scale-100"
          leave="transition ease-in duration-75"
          leaveFrom="opacity-100 scale-100"
          leaveTo="opacity-0 scale-95"
        >
          <ListboxOptions
            className="
             absolute mt-1 z-20 min-w-max w-48 py-1 rounded shadow
             bg-(--bg-popover)
             border border-(--border-color)
           "
          >
            {POLL_OPTIONS.map((opt) => (
              <ListboxOption
                key={opt.ms}
                value={opt.ms}
                className="
                  cursor-pointer select-none px-3 py-2 font-mono flex gap-4
                  text-(--text-primary)
                  data-[headlessui-state~=active]:bg-(--bg-popover-hover)
                "
              >
                <span className="w-12 text-right">{opt.label}</span>
                <span className="text-(--text-secondary) whitespace-nowrap">
                  {opt.desc}
                </span>
              </ListboxOption>
            ))}
          </ListboxOptions>
        </Transition>
      </div>
    </Listbox>
  )
}

export function RunsPage() {
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [filterJobId, setFilterJobId] = useState<string | undefined>(undefined)
  const [filterWorkerId, setFilterWorkerId] = useState<string | undefined>(
    undefined
  )
  const [pollMs, setPollMs] = useState(2000)

  // TODO: Option to not remove older runs from display, only add news ones. Try to not keep scrolling past where the user scrolled
  // TODO: Load more button at the bottom to see more history
  // TODO: Potential from/to date filters (and no refresh for them if "to" is past "now" maybe?)
  // TODO: Or maybe just pick a scheduled for time a bit before loading, and just query ones from then on

  const makeQuery = () => {
    const q: ListRunsQuery = {}
    if (filterJobId) {
      q.byJobId = filterJobId
    }
    if (filterWorkerId) {
      q.byWorkerId = filterWorkerId
    }

    q.limit = 100

    return q
  }
  const {
    data: runs,
    isLoading,
    error,
    refetch: refetchRuns,
  } = useRuns(makeQuery())
  const { data: jobs, error: jobsError } = useJobs()
  const { data: workers, error: workersError } = useWorkers()

  const getSelected = () => runs?.find((r) => r.id === selectedId) ?? null
  // const getJob = (job_id: string) => jobs?.find((job) => job.id === job_id)

  const jobsMap = useMemo(() => {
    const m = new Map()
    jobs?.forEach((j) => m.set(j.id, j))
    return m
  }, [jobs])

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-semibold">Recent Runs</h2>

      {isLoading && <p className="text-(--text-muted)">Loading…</p>}
      {error && <p className="text-(--text-danger)">{String(error)}</p>}
      {jobsError && <p className="text-(--text-danger)">{String(jobsError)}</p>}
      {workersError && (
        <p className="text-(--text-danger)">{String(workersError)}</p>
      )}

      <div className="flex gap-4 items-end">
        {/* Job filter */}
        <SearchableDropdown
          label="Job"
          items={[
            { value: '', label: '- All jobs -' },
            ...(jobs?.map((j) => ({ value: j.id, label: j.name })) ?? []),
          ]}
          value={filterJobId ?? ''}
          onChange={(val) => setFilterJobId(val || undefined)}
        />
        {/* Worker filter */}
        <SearchableDropdown
          label="Worker"
          items={[
            { value: '', label: '- All workers -' },
            ...(workers?.map((w) => ({
              value: w.id,
              label: `${w.hostname}-${w.id}`,
            })) ?? []),
          ]}
          value={filterWorkerId ?? ''}
          onChange={(val) => setFilterWorkerId(val || undefined)}
        />

        {/* Polling Frequency */}
        <div>
          <div className="flex gap-2 items-center">
            <PollSelect pollMs={pollMs} setPollMs={setPollMs} />
            {pollMs === 0 && (
              <button
                type="button"
                onClick={() => refetchRuns()}
                className="
                  bg-(--bg-button-secondary)
                  hover:bg-(--bg-button-secondary-hover)
                  text-(--text-primary) px-3 py-2 rounded text-sm
                "
              >
                Refresh Now
              </button>
            )}
          </div>
        </div>
      </div>

      {runs && (
        <div className="rounded-lg shadow border border-(--border-color) overflow-hidden bg-(--bg-surface-alt)">
          <table className="w-full text-left">
            <thead className="bg-(--bg-header) border-b border-(--border-subtle)">
              <tr>
                <th className="px-4 py-2 font-semibold">Job</th>
                <th className="px-4 py-2 font-semibold">State</th>
                <th className="px-4 py-2 font-semibold">Started</th>
                <th className="px-4 py-2 font-semibold">Finished</th>
                <th className="px-4 py-2 font-semibold">Scheduled for</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-(--border-subtle)">
              {runs.map((run) => (
                <tr
                  key={run.id}
                  className="hover:bg-(--bg-row-hover) cursor-pointer"
                  onClick={() => setSelectedId(run.id)}
                >
                  <td className="px-4 py-2">
                    <div>
                      <span>
                        {jobsMap.get(run.jobId)?.name ?? '<Unknown Job>'}
                      </span>
                      <div className="text-xs text-gray-500">
                        Worker: {run.workerId ?? '—'}
                      </div>
                    </div>
                  </td>

                  <td className="px-4 py-2">
                    <RunStateBadge state={run.state} runId={run.id} />
                  </td>
                  <td className="px-4 py-2">{formatTime(run.startedAt)}</td>
                  <td className="px-4 py-2">{formatTime(run.finishedAt)}</td>
                  <td className="px-4 py-2">{formatTime(run.scheduledFor)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <SlideOver
        open={!!getSelected()}
        onClose={() => setSelectedId(null)}
        title={`Run ${getSelected()?.id}`}
      >
        {getSelected() && <RunDetail run={getSelected()!} />}
      </SlideOver>
    </div>
  )
}

function formatTime(t?: string | null) {
  if (!t) return '—'
  return new Date(t).toLocaleString()
}

// function RunStateBadge({ state }: { state: JobRun['state'] }) {
//   const style =
//     state === 'succeeded'
//       ? 'bg-green-100 text-green-700'
//       : state === 'failed'
//       ? 'bg-red-100 text-red-700'
//       : state === 'running'
//       ? 'bg-blue-100 text-blue-700 dots'
//       : state === 'cancelled'
//       ? 'bg-gray-300 text-gray-700'
//       : 'bg-yellow-100 text-yellow-700'

//   return <span className={`px-2 py-1 rounded text-xs ${style}`}>{state}</span>
// }

function getRunningAnimationClass(id: string): string {
  let hash = 0
  for (let i = 0; i < id.length; i++) {
    hash = (hash * 31 + id.charCodeAt(i)) | 0
  }
  const idx = Math.abs(hash) % 3

  switch (idx) {
    case 0:
      // blue pulse (color + opacity)
      return 'animate-blue-pulse'
    case 1:
      // shimmering background
      return 'shimmer'
    case 2:
      // "running..." dots
      return 'dots'
    default:
      return 'animate-blue-pulse'
  }
}

function RunStateBadge({
  state,
  runId,
}: {
  state: JobRun['state']
  runId: JobRun['id']
}) {
  // TODO: Use switch ot something?
  const baseStyle =
    state === 'succeeded'
      ? 'bg-[var(--bg-success)] text-[var(--text-success)]'
      : state === 'failed'
        ? 'bg-[var(--bg-error)] text-[var(--text-error)]'
        : state === 'running'
          ? 'bg-[var(--bg-running)] text-[var(--text-running)]'
          : state === 'cancelled'
            ? 'bg-[var(--bg-neutral)] text-[var(--text-neutral)]'
            : 'bg-[var(--bg-warning)] text-[var(--text-warning)]'

  const runningAnimationClass =
    state === 'running' ? getRunningAnimationClass(runId) : ''

  return (
    <span
      key={runId} /* important: forces animation when state changes */
      className={`px-2 py-1 rounded text-xs transition-all duration-300 ease-in-out status-change ${baseStyle} ${runningAnimationClass}`}
    >
      {state}
    </span>
  )
}
