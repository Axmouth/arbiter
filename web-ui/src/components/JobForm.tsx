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
import { Cron } from "react-js-cron";
import cronstrue from "cronstrue";

type JobFormProps = {
    mode: 'create' | 'edit'
    initial?: JobSpec
    onComplete: (job: JobSpec) => void
    onCancel: () => void
}

type MisfirePolicyType =
    | 'skip'
    | 'run_immediately'
    | 'coalesce'
    | 'run_all'
    | 'run_if_late_within';

export function JobForm({ mode, initial, onComplete, onCancel }: JobFormProps) {
    const qc = useQueryClient()

    // Controlled inputs
    const [name, setName] = useState(initial?.name ?? '')
    const [cron, setCron] = useState(initial?.schedule_cron ?? '')
    const [command, setCommand] = useState(initial?.command ?? '')
    const [maxConcurrency, setMaxConcurrency] = useState(
        initial?.max_concurrency ?? 1
    )
    const [misfirePolicyType, setMisfirePolicyType] = useState<MisfirePolicyType>(
        initial?.misfire_policy ? inferMisfireType(initial.misfire_policy) as MisfirePolicyType : 'run_immediately'
    )

    const [misfireDuration, setMisfireDuration] = useState<number>(
        initial?.misfire_policy ? inferMisfireDuration(initial.misfire_policy) : 0
    );
    const [cronMode, setCronMode] = useState<"builder" | "text">("builder");

    let mutationFn: () => Promise<JobSpec>;
    if (mode === 'create') {
        mutationFn = async () => {
            let misfire_policy: MisfirePolicy;

            if (misfirePolicyType === "run_if_late_within") {
                misfire_policy = { run_if_late_within: [misfireDuration, 0] };
            } else {
                misfire_policy = misfirePolicyType;
            }

            const payload: CreateJobRequest = {
                name,
                schedule_cron: cron || null,
                command,
                max_concurrency: maxConcurrency,
                misfire_policy: misfire_policy,
            }

            return await createJob(payload)
        }
    } else {
        mutationFn = async () => {
            let misfire_policy: MisfirePolicy;

            if (misfirePolicyType === "run_if_late_within") {
                misfire_policy = { run_if_late_within: [misfireDuration, 0] };
            } else {
                misfire_policy = misfirePolicyType;
            }

            const payload: UpdateJobRequest = {
                name,
                schedule_cron: cron || null,
                command,
                max_concurrency: maxConcurrency,
                misfire_policy: misfire_policy,
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

    return (
        <form
            onSubmit={(e) => {
                e.preventDefault()
                mutation.mutate()
            }}
            className="space-y-6"
        >
            {/* Name */}
            <div>
                <label className="block text-sm font-medium">
                    Name
                </label>
                <input
                    type="text"
                    required
                    className="mt-1 w-full rounded border px-3 py-2"
                    value={name}
                    onChange={(e) =>
                        setName(e.target.value)
                    }
                />
            </div>

            {/* Cron */}
            <div>
                <label className="block text-sm font-medium">Cron (schedule)</label>

                <div className="mt-2">
                    {cronMode === "builder" ? (
                    <Cron value={cron} setValue={setCron} />
                    ) : (
                    <input
                        type="text"
                        className="mt-1 w-full rounded border px-3 py-2"
                        value={cron}
                        onChange={(e) => setCron(e.target.value)}
                    />
                    )}
                </div>

                {cron && (
                    <p className="text-sm text-gray-500 mt-1">
                    {cronstrue.toString(cron)}
                    </p>
                )}
                </div>

            <div className="flex gap-2 items-center">
                <button
                    type="button"
                    className={`px-2 py-1 rounded ${cronMode === "builder" ? "bg-blue-600 text-white" : "bg-gray-200"}`}
                    onClick={() => setCronMode("builder")}
                >
                    Builder
                </button>

                <button
                    type="button"
                    className={`px-2 py-1 rounded ${cronMode === "text" ? "bg-blue-600 text-white" : "bg-gray-200"}`}
                    onClick={() => setCronMode("text")}
                >
                    Text
                </button>
                </div>

            {/* Command */}
            <div>
                <label className="block text-sm font-medium">
                    Command
                </label>
                <textarea
                    className="mt-1 w-full rounded border px-3 py-2"
                    value={command}
                    onChange={(e) =>
                        setCommand(e.target.value)
                    }
                />
            </div>

            {/* Concurrency */}
            <div>
                <label className="block text-sm font-medium">
                    Max Concurrency
                </label>
                <input
                    type="number"
                    min={1}
                    className="mt-1 w-20 rounded border px-3 py-2"
                    value={maxConcurrency}
                    onChange={(e) =>
                        setMaxConcurrency(
                            Number(e.target.value)
                        )
                    }
                />
            </div>

            {/* Misfire policy */}
            <div>
                <label className="block text-sm font-medium">
                    Misfire Policy
                </label>
                <select
                    className="mt-1 w-full rounded border px-3 py-2"
                    value={
                        misfirePolicyType
                    }
                    onChange={(e) =>
                        setMisfirePolicyType(
                                e.target.value as MisfirePolicyType
                        )
                    }
                >
                    <option value="skip">Skip</option>
                    <option value="run_immediately">
                        Run Immediately
                    </option>
                    <option value="coalesce">
                        Coalesce
                    </option>
                    <option value="run_all">Run All</option>
                    <option value="run_if_late_within">Run If Late (duration)</option>
                                    </select>
                                </div>

                                {misfirePolicyType === "run_if_late_within" && (
                    <div>
                        <label className="block text-sm font-medium">Late Within (seconds)</label>
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
                <p className="text-red-600">
                    {String(mutation.error)}
                </p>
            )}

            {/* Submit */}
            <button
                type="submit"
                disabled={mutation.isPending}
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
