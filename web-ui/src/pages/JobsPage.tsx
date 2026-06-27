import { useState } from 'react'
import { useJobs } from '../hooks/useJobs'
import { useChangeStream } from '../hooks/useChangeStream'
import { SlideOver } from '../components/SlideOver'
import type { JobSpec } from '../backend-types/JobSpec'
import { JobForm } from '../components/JobForm'
import { JobDetailsView } from './JobDetail'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { deleteJob, disableJob, enableJob, runJobNow } from '../api/jobs'

export function JobsPage() {
  const { data: jobs, isLoading, error } = useJobs()
  // Live updates on job create/edit/enable/delete via the jobs change-stream.
  useChangeStream('/api/v1/jobs/stream', 'jobs')
  const [selectedJob, setSelectedJob] = useState<JobSpec | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [editMode, setEditMode] = useState(false)
  const qc = useQueryClient()
  const [detailsOpen, setDetailsOpen] = useState(false)
  const runNowMutation = useMutation({
    mutationFn: () => runJobNow(selectedJob!.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['runs'] })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: () => deleteJob(selectedJob!.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['jobs'] })
      setDetailsOpen(false)
    },
  })

  const toggleEnabledMutation = useMutation({
    mutationFn: () =>
      selectedJob!.enabled
        ? disableJob(selectedJob!.id)
        : enableJob(selectedJob!.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['jobs'] })
      if (selectedJob) {
        setSelectedJob((prev) =>
          prev
            ? {
                ...prev,
                enabled: !prev.enabled,
              }
            : prev
        )
      }
    },
  })

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold text-(--text-primary)">Jobs</h2>

      <button
        onClick={() => setCreateOpen(true)}
        className="
          bg-(--bg-btn-primary) text-(--text-inverse) border border-(--border-color) text-[13px]
          px-3 py-1.5 rounded
          hover:bg-(--bg-btn-primary-hover)
        "
      >
        New Job
      </button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {jobs && (
        <div
          className="
              rounded-lg border border-(--border-color)
              overflow-hidden bg-(--bg-surface-alt)
            "
        >
          <table className="w-full text-left">
            <thead className="bg-(--bg-header) text-(--text-primary) border-b border-(--border-subtle)">
              <tr>
                <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)">Name</th>
                <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)">Enabled</th>
                <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)">Cron</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-(--border-subtle)">
              {jobs.map((job) => (
                <tr
                  key={job.id}
                  className="
                    hover:bg-(--bg-row-hover)
                    cursor-pointer text-(--text-primary)
                  "
                  onClick={() => {
                    setSelectedJob(job)
                    setDetailsOpen(true)
                  }}
                >
                  <td className="px-3 py-1.5">{job.name}</td>
                  <td className="px-3 py-1.5">
                    <span
                      className={`
                       inline-block px-2 py-1 text-xs rounded
                       ${
                         job.enabled
                           ? 'bg-(--bg-success) text-(--text-success)'
                           : 'bg-(--bg-error) text-(--text-error)'
                       }
                     `}
                    >
                      {job.enabled ? 'enabled' : 'disabled'}
                    </span>
                  </td>
                  <td className="px-3 py-1.5">{job.scheduleCron ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <SlideOver
        open={detailsOpen}
        wide
        onClose={() => {
          setDetailsOpen(false)
          setEditMode(false)
          setCreateOpen(false)
        }}
        title={selectedJob?.name ?? ''}
      >
        {editMode ? (
          <JobForm
            mode="edit"
            existingJobs={jobs ?? []}
            initial={selectedJob!}
            onComplete={(job: JobSpec) => {
              setSelectedJob(job)
            }}
            onCancel={() => {
              setEditMode(false)
              setCreateOpen(false)
            }}
          />
        ) : (
          <JobDetailsView
            job={selectedJob!}
            onEdit={() => setEditMode(true)}
            onToggleEnabled={() => {
              toggleEnabledMutation.mutate()
              qc.invalidateQueries({
                queryKey: ['jobs'],
              })
            }}
            onRunNow={() => {
              runNowMutation.mutate()
            }}
            onDelete={() => {
              if (confirm('Delete this job?')) {
                deleteMutation.mutate()
                setDetailsOpen(false)
                qc.invalidateQueries({
                  queryKey: ['jobs'],
                })
              }
            }}
            onComplete={() => {
              setDetailsOpen(false)
              setEditMode(false)
            }}
          />
        )}
      </SlideOver>

      <SlideOver
        open={createOpen}
        wide
        onClose={() => {
          setCreateOpen(false)
          setEditMode(false)
        }}
        title="Create Job"
      >
        <JobForm
          mode="create"
          existingJobs={jobs ?? []}
          onComplete={() => {
            setCreateOpen(false)
            setEditMode(false)
          }}
          onCancel={() => setCreateOpen(false)}
        />
      </SlideOver>
    </div>
  )
}
