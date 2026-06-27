import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type {
  CreateJobRequest,
  JobSpec,
  MisfirePolicy,
  RunnerConfig,
  UpdateJobRequest,
} from '../backend-types'
import { inferMisfireDuration, inferMisfireType } from '../utils/misfire'
import { createJob, fetchJobEnv, updateJob } from '../api/jobs'
import { useDbConfigs } from '../hooks/useDbConfigs'
import { RunnerConfigFields } from './RunnerConfigFields'
import { defaultRunner, isRunnerValid } from '../utils/runner'
import { KeyValueEditor } from './KeyValueEditor'
import { pairsToRecord, recordToPairs, type KvPair } from '../utils/keyvalue'
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

const inputCls =
  'mt-1 w-full rounded border border-(--border-color) bg-(--bg-app) text-(--text-primary) px-3 py-1.5'

export function JobForm({
  mode,
  initial,
  existingJobs,
  onComplete,
  onCancel,
}: JobFormProps) {
  const qc = useQueryClient()
  const { data: dbConfigs } = useDbConfigs()

  const [name, setName] = useState(initial?.name ?? '')
  const [cron, setCron] = useState(initial?.scheduleCron ?? '')
  const [runner, setRunner] = useState<RunnerConfig>(
    initial?.runnerCfg ?? defaultRunner('shell')
  )
  // `null` = not yet edited, so the editor reflects the loaded env until the user
  // touches it (avoids a setState-in-effect just to seed it).
  const [envEdits, setEnvEdits] = useState<KvPair[] | null>(null)
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

  // Edit mode: env is not part of JobSpec, so load it separately.
  const { data: loadedEnv } = useQuery({
    queryKey: ['job-env', initial?.id],
    queryFn: () => fetchJobEnv(initial!.id),
    enabled: mode === 'edit' && !!initial?.id,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  })
  const envPairs = envEdits ?? recordToPairs(loadedEnv)

  function validateCron(value: string) {
    if (!value) {
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

  function buildMisfire(): MisfirePolicy {
    if (misfirePolicyType === 'runIfLateWithin') {
      return { runIfLateWithin: [misfireDuration, 0] }
    }
    return misfirePolicyType
  }

  const mutation = useMutation({
    mutationFn: async () => {
      if (mode === 'create') {
        const payload: CreateJobRequest = {
          name,
          scheduleCron: cron || null,
          runnerConfig: runner,
          maxConcurrency,
          misfirePolicy: buildMisfire(),
          retry: null,
          env: pairsToRecord(envPairs),
        }
        return await createJob(payload)
      }
      const payload: UpdateJobRequest = {
        name,
        scheduleCron: cron || null,
        runnerConfig: runner,
        maxConcurrency,
        misfirePolicy: buildMisfire(),
        retry: null,
        env: pairsToRecord(envPairs) ?? {},
      }
      return await updateJob(initial!.id, payload)
    },
    onSuccess: (job: JobSpec) => {
      qc.invalidateQueries({ queryKey: ['jobs'] })
      if (mode === 'edit' && initial?.id) {
        qc.invalidateQueries({ queryKey: ['job-env', initial.id] })
      }
      onComplete(job)
    },
  })

  const currentId = initial?.id
  const hasDuplicateName =
    !!name.trim() &&
    existingJobs.some((job) => job.name === name && job.id !== currentId)

  const canSubmit =
    !!name.trim() && !cronError && isRunnerValid(runner) && !mutation.isPending

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
          className={inputCls}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        {!name.trim() && (
          <p className="text-sm text-(--text-danger) mt-1">Name is required</p>
        )}
        {!!name.trim() && hasDuplicateName && (
          <p className="text-sm text-(--text-warning) mt-1">
            A job with this name already exists.
          </p>
        )}
      </div>

      <Section title="Schedule">
        <div className="mt-2">
          {cronMode === 'builder' ? (
            <Cron
              value={cron}
              setValue={(value: string) => {
                setCron(value)
                validateCron(value)
              }}
              className="my-cron"
            />
          ) : (
            <input
              type="text"
              className={inputCls}
              value={cron}
              onChange={(e) => {
                setCron(e.target.value)
                validateCron(e.target.value)
              }}
            />
          )}
        </div>

        {cron && !cronError && (
          <p className="text-sm text-(--text-secondary) mt-1">
            {cronstrue.toString(cron)}
          </p>
        )}
        {cronError && (
          <p className="text-sm text-(--text-danger) mt-1">{cronError}</p>
        )}
        {!cron && (
          <p className="text-sm text-(--text-muted) mt-1">
            No schedule — the job only runs on demand.
          </p>
        )}

        <div className="flex gap-2 items-center mt-2">
          <ToggleButton
            active={cronMode === 'builder'}
            onClick={() => setCronMode('builder')}
          >
            Builder
          </ToggleButton>
          <ToggleButton
            active={cronMode === 'text'}
            onClick={() => setCronMode('text')}
          >
            Text
          </ToggleButton>
        </div>
      </Section>

      <Section title="Runner">
        <RunnerConfigFields
          initial={initial?.runnerCfg}
          onChange={setRunner}
          dbConfigs={dbConfigs ?? []}
        />
      </Section>

      <Section title="Environment">
        <p className="text-sm text-(--text-muted) mb-2">
          Injected into the runner. A value of{' '}
          <code className="text-(--text-primary)">secret:&lt;name&gt;</code> is
          resolved from a secret at execution.
        </p>
        <KeyValueEditor
          pairs={envPairs}
          onChange={setEnvEdits}
          addLabel="Add variable"
        />
      </Section>

      <Section title="Execution">
        <div>
          <label className="block text-sm font-medium">Max concurrency</label>
          <input
            type="number"
            min={1}
            className="mt-1 w-24 rounded border border-(--border-color) bg-(--bg-app) text-(--text-primary) px-3 py-1.5"
            value={maxConcurrency}
            onChange={(e) => setMaxConcurrency(Number(e.target.value))}
          />
        </div>

        <div className="mt-4">
          <label className="block text-sm font-medium">Misfire policy</label>
          <select
            className={inputCls}
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
          <div className="mt-4">
            <label className="block text-sm font-medium">
              Late within (seconds)
            </label>
            <input
              type="number"
              min={0}
              value={misfireDuration}
              onChange={(e) => setMisfireDuration(Number(e.target.value))}
              className={inputCls}
            />
          </div>
        )}
      </Section>

      {mutation.isError && (
        <p className="text-(--text-danger)">{String(mutation.error)}</p>
      )}

      <div className="flex gap-3">
        <button
          type="submit"
          disabled={!canSubmit}
          className="bg-(--text-accent) text-(--text-inverse) px-3 py-1.5 rounded hover:bg-(--text-accent-hover) disabled:opacity-50"
        >
          {mode === 'create' ? 'Create Job' : 'Save'}
        </button>
        <button
          type="button"
          onClick={() => onCancel()}
          className="bg-(--bg-button-secondary) text-(--text-primary) px-3 py-1.5 rounded hover:bg-(--bg-button-secondary-hover)"
        >
          Cancel
        </button>
      </div>
    </form>
  )
}

function Section({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <fieldset className="border-t border-(--border-subtle) pt-4">
      <legend className="text-sm font-semibold text-(--text-secondary) pr-2">
        {title}
      </legend>
      {children}
    </fieldset>
  )
}

function ToggleButton({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`px-2 py-1 rounded ${
        active
          ? 'bg-(--text-accent) text-(--text-inverse)'
          : 'bg-(--bg-button-secondary) text-(--text-primary) hover:bg-(--bg-button-secondary-hover)'
      }`}
    >
      {children}
    </button>
  )
}
