import type {
  CreateJobRequest,
  JobRun,
  UpdateJobRequest,
} from '../backend-types'
import type { JobSpec } from '../backend-types/JobSpec'
import { api } from './client'

export function fetchJobs(): Promise<JobSpec[]> {
  return api<JobSpec[]>('/jobs')
}

export function fetchJob(id: string): Promise<JobSpec> {
  return api<JobSpec>(`/jobs/${id}`)
}

export function deleteJob(id: string): Promise<void> {
  return api<void>(`/jobs/${id}`, { method: 'DELETE' })
}

export function runJobNow(id: string): Promise<JobRun> {
  return api<JobRun>(`/jobs/${id}/run`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
  })
}

export function createJob(job: CreateJobRequest): Promise<JobSpec> {
  return api<JobSpec>('/jobs', {
    method: 'POST',
    body: JSON.stringify(job),
    headers: { 'Content-Type': 'application/json' },
  })
}

export function updateJob(id: string, job: UpdateJobRequest): Promise<JobSpec> {
  return api<JobSpec>(`/jobs/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(job),
    headers: { 'Content-Type': 'application/json' },
  })
}

export function enableJob(id: string): Promise<JobSpec> {
  return api<JobSpec>(`/jobs/${id}/enable`, {
    method: 'POST',
  })
}

export function disableJob(id: string): Promise<JobSpec> {
  return api<JobSpec>(`/jobs/${id}/disable`, {
    method: 'POST',
  })
}
