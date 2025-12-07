import { useState } from 'react'
import { useJobs } from '../hooks/useJobs'
import { useRuns } from '../hooks/useRuns'
import { useWorkers } from '../hooks/useWorkers'
import { Link } from '@tanstack/react-router'

export function HomePage() {
  // Frozen at mount
  const [pageLoadTime] = useState(() => Date.now())
  const [afterISO] = useState(() =>
    new Date(pageLoadTime - 60 * 60 * 1000).toISOString()
  )

  const { data: jobs } = useJobs()
  const { data: runs } = useRuns({ after: afterISO })
  const { data: workers } = useWorkers()

  const enabledJobs = jobs?.filter((j) => j.enabled).length ?? 0
  const disabledJobs = jobs ? jobs.length - enabledJobs : 0

  const recentRuns = runs ?? []
  const succeeded = recentRuns.filter((r) => r.state === 'succeeded').length
  const failed = recentRuns.filter((r) => r.state === 'failed').length
  const inProgress = recentRuns.filter((r) =>
    ['running', 'queued'].includes(r.state)
  ).length

  const onlineThreshold = 20_000 // 20s
  const online =
    workers?.filter(
      (w) => pageLoadTime - new Date(w.lastSeen).getTime() < onlineThreshold
    ).length ?? 0
  const offline = workers ? workers.length - online : 0

  return (
    <div className="space-y-8">
      <h1 className="text-2xl font-semibold text-(--text-primary)">
        Dashboard
      </h1>

      <div className="grid gap-6 grid-cols-1 md:grid-cols-3">
        {/* Jobs */}
        <StatsCard
          title="Jobs"
          value={jobs?.length ?? '—'}
          detail={`${enabledJobs} enabled, ${disabledJobs} disabled`}
          link="/jobs"
        />

        {/* Workers */}
        <StatsCard
          title="Workers"
          value={workers?.length ?? '—'}
          detail={`${online} online, ${offline} offline`}
          link="/workers"
        />

        {/* Recent runs */}
        <StatsCard
          title="Recent Runs"
          value={recentRuns.length}
          detail={`${succeeded} ok, ${failed} failed, ${inProgress} in progress`}
          link="/runs"
        />
      </div>

      <div className="pt-4 flex gap-4">
        <Link
          to="/jobs"
          className="
            px-4 py-2 rounded
            bg-(--bg-btn-primary) text-(--text-inverse)
            hover:bg-(--bg-btn-primary-hover)
          "
        >
          View Jobs
        </Link>
        <Link
          to="/jobs"
          className="
            px-4 py-2 rounded
            bg-(--bg-btn-secondary) text-(--text-primary)
            hover:bg-(--bg-btn-secondary-hover)
          "
        >
          Create Job
        </Link>
      </div>
    </div>
  )
}

function StatsCard({
  title,
  value,
  detail,
  link,
}: {
  title: string
  value: number | string
  detail: string
  link: string
}) {
  return (
    <Link
      to={link}
      className="
        bg-(--bg-card) text-(--text-primary)
        border border-(--border-subtle)
        shadow rounded-lg p-5 flex flex-col gap-2
        hover:shadow-md transition-shadow
      "
    >
      <span className="text-sm font-medium">{title}</span>
      <span className="text-3xl font-bold">{value}</span>
      <span className="text-sm text-(--text-secondary)">{detail}</span>
    </Link>
  )
}
