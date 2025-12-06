import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import type {
  CreateJobRequest,
  JobSpec,
  MisfirePolicy,
  UpdateJobRequest,
} from '../backend-types'
import { inferMisfireDuration, inferMisfireType } from '../utils/misfire'
import { createJob, updateJob } from '../api/jobs'
import { Cron } from 'react-js-cron'
import cronstrue from 'cronstrue'

type JobFormProps = {
  mode: 'create' | 'edit'
  initial?: JobSpec
  existingJobs: JobSpec[]
  onComplete: (job: JobSpec) => void
  onCancel: () => void
}

type MisfirePolicyType =
  | 'skip'
  | 'runImmediately'
  | 'coalesce'
  | 'runAll'
  | 'runIfLateWithin'

export function JobForm({
  mode,
  initial,
  existingJobs,
  onComplete,
  onCancel,
}: JobFormProps) {
  const qc = useQueryClient()

  // Controlled inputs
  const [name, setName] = useState(initial?.name ?? '')
  const [cron, setCron] = useState(initial?.scheduleCron ?? '')
  const [command, setCommand] = useState(
    initial?.runnerCfg.type === 'shell' ? initial?.runnerCfg.command : ''
  )
  const [maxConcurrency, setMaxConcurrency] = useState(
    initial?.maxConcurrency ?? 1
  )
  const [misfirePolicyType, setMisfirePolicyType] = useState<MisfirePolicyType>(
    initial?.misfirePolicy
      ? (inferMisfireType(initial.misfirePolicy) as MisfirePolicyType)
      : 'runImmediately'
  )

  const [misfireDuration, setMisfireDuration] = useState<number>(
    initial?.misfirePolicy ? inferMisfireDuration(initial.misfirePolicy) : 0
  )
  const [cronMode, setCronMode] = useState<'builder' | 'text'>('builder')
  const [cronError, setCronError] = useState<string | null>(null)
  const [nameError, setNameError] = useState<string | null>(null)
  const [commandError, setCommandError] = useState<string | null>(null)

  function validateCron(value: string) {
    if (!value) {
      // Empty cron is allowed in your payload (you send null), so no error.
      setCronError(null)
      return
    }

    try {
      cronstrue.toString(value)
      setCronError(null)
    } catch {
      setCronError('Invalid or incomplete cron expression')
    }
  }

  let mutationFn: () => Promise<JobSpec>
  if (mode === 'create') {
    mutationFn = async () => {
      let misfirePolicy: MisfirePolicy

      if (misfirePolicyType === 'runIfLateWithin') {
        misfirePolicy = { runIfLateWithin: [misfireDuration, 0] }
      } else {
        misfirePolicy = misfirePolicyType
      }

      const payload: CreateJobRequest = {
        name,
        scheduleCron: cron || null,
        runnerConfig: { type: 'shell', command: command, workingDir: null },
        maxConcurrency: maxConcurrency,
        misfirePolicy: misfirePolicy,
      }

      return await createJob(payload)
    }
  } else {
    mutationFn = async () => {
      let misfirePolicy: MisfirePolicy

      if (misfirePolicyType === 'runIfLateWithin') {
        misfirePolicy = { runIfLateWithin: [misfireDuration, 0] }
      } else {
        misfirePolicy = misfirePolicyType
      }

      const payload: UpdateJobRequest = {
        name,
        scheduleCron: cron || null,
        runnerConfig: { type: 'shell', command: command, workingDir: null },
        maxConcurrency: maxConcurrency,
        misfirePolicy: misfirePolicy,
      }

      return await updateJob(initial!.id, payload)
    }
  }

  const mutation = useMutation({
    mutationFn,
    onSuccess: (job: JobSpec) => {
      qc.invalidateQueries({ queryKey: ['jobs'] })
      onComplete(job)
    },
  })

  const currentId = initial?.id
  const hasDuplicateName =
    !!name.trim() &&
    existingJobs.some((job) => job.name === name && job.id !== currentId)

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        if (hasDuplicateName) {
          const ok = window.confirm(
            'A job with this name already exists. Proceed anyway?'
          )
          if (!ok) return
        }

        mutation.mutate()
      }}
      className="space-y-6"
    >
      {/* Name */}
      <div>
        <label className="block text-sm font-medium">Name</label>
        <input
          type="text"
          required
          className="mt-1 w-full rounded border px-3 py-2"
          value={name}
          onChange={(e) => {
            const value = e.target.value
            setName(value)
            if (!value.trim()) {
              setNameError('Name is required')
            } else {
              setNameError(null)
            }
          }}
        />
      </div>

      {nameError && <p className="text-sm text-red-600 mt-1">{nameError}</p>}

      {!nameError && hasDuplicateName && (
        <p className="text-sm text-amber-600 mt-1">
          A job with this name already exists.
        </p>
      )}

      {/* Cron */}
      <div>
        <label className="block text-sm font-medium">Cron (schedule)</label>

        <div className="mt-2">
          {cronMode === 'builder' ? (
            <Cron
              value={cron}
              setValue={(value: string) => {
                setCron(value)
                validateCron(value)
              }}
            />
          ) : (
            <input
              type="text"
              className="mt-1 w-full rounded border px-3 py-2"
              value={cron}
              onChange={(e) => {
                const value = e.target.value
                setCron(value)
                validateCron(value)
              }}
            />
          )}
        </div>

        {cron && !cronError && (
          <p className="text-sm text-gray-500 mt-1">
            {cronstrue.toString(cron)}
          </p>
        )}

        {cronError && <p className="text-sm text-red-600 mt-1">{cronError}</p>}
      </div>

      <div className="flex gap-2 items-center">
        <button
          type="button"
          className={`px-2 py-1 rounded ${
            cronMode === 'builder' ? 'bg-blue-600 text-white' : 'bg-gray-200'
          }`}
          onClick={() => setCronMode('builder')}
        >
          Builder
        </button>

        <button
          type="button"
          className={`px-2 py-1 rounded ${
            cronMode === 'text' ? 'bg-blue-600 text-white' : 'bg-gray-200'
          }`}
          onClick={() => setCronMode('text')}
        >
          Text
        </button>
      </div>

      {/* Command */}
      <div>
        <label className="block text-sm font-medium">Command</label>
        <textarea
          className="mt-1 w-full rounded border px-3 py-2"
          value={command}
          onChange={(e) => {
            const value = e.target.value
            setCommand(value)
            if (!value.trim()) {
              setCommandError('Command is required')
            } else {
              setCommandError(null)
            }
          }}
        />
      </div>

      {commandError && (
        <p className="text-sm text-red-600 mt-1">{commandError}</p>
      )}

      {/* Concurrency */}
      <div>
        <label className="block text-sm font-medium">Max Concurrency</label>
        <input
          type="number"
          min={1}
          className="mt-1 w-20 rounded border px-3 py-2"
          value={maxConcurrency}
          onChange={(e) => setMaxConcurrency(Number(e.target.value))}
        />
      </div>

      {/* Misfire policy */}
      <div>
        <label className="block text-sm font-medium">Misfire Policy</label>
        <select
          className="mt-1 w-full rounded border px-3 py-2"
          value={misfirePolicyType}
          onChange={(e) =>
            setMisfirePolicyType(e.target.value as MisfirePolicyType)
          }
        >
          <option value="skip">Skip</option>
          <option value="runImmediately">Run Immediately</option>
          <option value="coalesce">Coalesce</option>
          <option value="runAll">Run All</option>
          <option value="runIfLateWithin">Run If Late (duration)</option>
        </select>
      </div>

      {misfirePolicyType === 'runIfLateWithin' && (
        <div>
          <label className="block text-sm font-medium">
            Late Within (seconds)
          </label>
          <input
            type="number"
            min={0}
            value={misfireDuration}
            onChange={(e) => setMisfireDuration(Number(e.target.value))}
            className="mt-1 w-full rounded border px-3 py-2"
          />
        </div>
      )}

      {/* Error */}
      {mutation.isError && (
        <p className="text-red-600">{String(mutation.error)}</p>
      )}

      {/* Submit */}
      <button
        type="submit"
        disabled={
          mutation.isPending ||
          !!cronError ||
          !!nameError ||
          !!commandError ||
          !name.trim() ||
          !command.trim()
        }
        className="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 disabled:opacity-50"
      >
        {mode === 'create' ? 'Create Job' : 'Save'}
      </button>

      {/* Cancel */}
      <button
        type="button"
        onClick={() => onCancel()}
        className="ml-3 bg-gray-300 text-gray-700 px-4 py-2 rounded hover:bg-gray-400"
      >
        Cancel
      </button>
    </form>
  )
}
