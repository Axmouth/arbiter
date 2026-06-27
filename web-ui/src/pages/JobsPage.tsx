import { useState } from 'react'
import { useJobs } from '../hooks/useJobs'
import { useChangeStream } from '../hooks/useChangeStream'
import { SlideOver } from '../components/SlideOver'
import type { JobSpec } from '../backend-types/JobSpec'
import { JobForm } from '../components/JobForm'
import { Button } from '../components/Button'
import { Table, THead, Th, TBody, Tr, Td } from '../components/Table'
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

      <Button variant="primary" onClick={() => setCreateOpen(true)}>
        New Job
      </Button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {jobs && (
        <Table>
          <THead>
            <Th>Name</Th>
            <Th>Enabled</Th>
            <Th>Cron</Th>
          </THead>
          <TBody>
            {jobs.map((job) => (
              <Tr
                key={job.id}
                onClick={() => {
                  setSelectedJob(job)
                  setDetailsOpen(true)
                }}
              >
                <Td>{job.name}</Td>
                <Td>
                  <span
                    className={`inline-block px-2 py-1 text-xs rounded ${
                      job.enabled
                        ? 'bg-(--bg-success) text-(--text-success)'
                        : 'bg-(--bg-error) text-(--text-error)'
                    }`}
                  >
                    {job.enabled ? 'enabled' : 'disabled'}
                  </span>
                </Td>
                <Td>{job.scheduleCron ?? '—'}</Td>
              </Tr>
            ))}
          </TBody>
        </Table>
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
