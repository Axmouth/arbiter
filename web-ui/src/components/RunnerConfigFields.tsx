import { useState } from 'react'
import type { RunnerConfig, SharedDbConfig } from '../backend-types'
import { KeyValueEditor } from './KeyValueEditor'
import { pairsToRecord, recordToPairs, type KvPair } from '../utils/keyvalue'
import { defaultRunner, RUNNER_LABELS, type RunnerType } from '../utils/runner'

const inputCls =
  'mt-1 w-full rounded border border-(--border-color) bg-(--bg-app) text-(--text-primary) px-3 py-1.5'

type Props = {
  initial?: RunnerConfig
  onChange: (cfg: RunnerConfig) => void
  dbConfigs: SharedDbConfig[]
}

export function RunnerConfigFields({ initial, onChange, dbConfigs }: Props) {
  const [cfg, setCfg] = useState<RunnerConfig>(
    initial ?? defaultRunner('shell')
  )
  const [headerPairs, setHeaderPairs] = useState<KvPair[]>(
    initial?.type === 'http' ? recordToPairs(initial.headers) : []
  )

  function emit(next: RunnerConfig) {
    setCfg(next)
    onChange(next)
  }

  function changeType(type: RunnerType) {
    setHeaderPairs([])
    emit(defaultRunner(type))
  }

  return (
    <div className="space-y-4">
      <div>
        <label className="block text-sm font-medium">Runner</label>
        <select
          className={inputCls}
          value={cfg.type}
          onChange={(e) => changeType(e.target.value as RunnerType)}
        >
          {(Object.keys(RUNNER_LABELS) as RunnerType[]).map((t) => (
            <option key={t} value={t}>
              {RUNNER_LABELS[t]}
            </option>
          ))}
        </select>
      </div>

      {cfg.type === 'shell' && (
        <>
          <Field label="Command">
            <textarea
              className={inputCls}
              rows={3}
              value={cfg.command}
              onChange={(e) => emit({ ...cfg, command: e.target.value })}
            />
          </Field>
          <Field label="Working directory (optional)">
            <input
              type="text"
              className={inputCls}
              value={cfg.workingDir ?? ''}
              onChange={(e) =>
                emit({ ...cfg, workingDir: e.target.value || null })
              }
            />
          </Field>
        </>
      )}

      {cfg.type === 'http' && (
        <>
          <Field label="Method">
            <select
              className={inputCls}
              value={cfg.method}
              onChange={(e) => emit({ ...cfg, method: e.target.value })}
            >
              {['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD'].map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </Field>
          <Field label="URL">
            <input
              type="text"
              className={inputCls}
              placeholder="https://example.com/hook"
              value={cfg.url}
              onChange={(e) => emit({ ...cfg, url: e.target.value })}
            />
          </Field>
          <Field label="Headers (optional)">
            <KeyValueEditor
              pairs={headerPairs}
              onChange={(p) => {
                setHeaderPairs(p)
                emit({ ...cfg, headers: pairsToRecord(p) })
              }}
              keyPlaceholder="Header"
              valuePlaceholder="value"
              addLabel="Add header"
            />
          </Field>
          <Field label="Body (optional)">
            <textarea
              className={inputCls}
              rows={3}
              value={cfg.body ?? ''}
              onChange={(e) => emit({ ...cfg, body: e.target.value || null })}
            />
          </Field>
          <TimeoutField
            value={cfg.timeoutSec}
            onChange={(v) => emit({ ...cfg, timeoutSec: v })}
          />
        </>
      )}

      {(cfg.type === 'pgSql' || cfg.type === 'mySql') && (
        <>
          <DbConfigPicker
            engine={cfg.type}
            value={cfg.configId}
            dbConfigs={dbConfigs}
            onChange={(id) => emit({ ...cfg, configId: id })}
          />
          <Field label="Query">
            <textarea
              className={`${inputCls} font-mono`}
              rows={4}
              value={cfg.query}
              onChange={(e) => emit({ ...cfg, query: e.target.value })}
            />
          </Field>
          <TimeoutField
            value={cfg.timeoutSec}
            onChange={(v) => emit({ ...cfg, timeoutSec: v })}
          />
        </>
      )}

      {cfg.type === 'python' && (
        <>
          <Field label="Module">
            <input
              type="text"
              className={inputCls}
              placeholder="my_pkg.tasks"
              value={cfg.module}
              onChange={(e) => emit({ ...cfg, module: e.target.value })}
            />
          </Field>
          <Field label="Class name">
            <input
              type="text"
              className={inputCls}
              value={cfg.className}
              onChange={(e) => emit({ ...cfg, className: e.target.value })}
            />
          </Field>
          <TimeoutField
            value={cfg.timeoutSec}
            onChange={(v) => emit({ ...cfg, timeoutSec: v })}
          />
        </>
      )}

      {cfg.type === 'node' && (
        <>
          <Field label="Module">
            <input
              type="text"
              className={inputCls}
              placeholder="./tasks.js"
              value={cfg.module}
              onChange={(e) => emit({ ...cfg, module: e.target.value })}
            />
          </Field>
          <Field label="Function name">
            <input
              type="text"
              className={inputCls}
              value={cfg.functionName}
              onChange={(e) => emit({ ...cfg, functionName: e.target.value })}
            />
          </Field>
          <TimeoutField
            value={cfg.timeoutSec}
            onChange={(v) => emit({ ...cfg, timeoutSec: v })}
          />
        </>
      )}
    </div>
  )
}

function Field({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div>
      <label className="block text-sm font-medium">{label}</label>
      {children}
    </div>
  )
}

function TimeoutField({
  value,
  onChange,
}: {
  value: number | null
  onChange: (v: number | null) => void
}) {
  return (
    <Field label="Timeout seconds (optional)">
      <input
        type="number"
        min={1}
        className={inputCls}
        value={value ?? ''}
        onChange={(e) =>
          onChange(e.target.value === '' ? null : Number(e.target.value))
        }
      />
    </Field>
  )
}

function DbConfigPicker({
  engine,
  value,
  dbConfigs,
  onChange,
}: {
  engine: 'pgSql' | 'mySql'
  value: string
  dbConfigs: SharedDbConfig[]
  onChange: (id: string) => void
}) {
  const options = dbConfigs.filter((c) => c.engine === engine)
  return (
    <Field label="Connection config">
      {options.length === 0 ? (
        <p className="text-xs text-(--text-danger) mt-1">
          No {engine === 'pgSql' ? 'PostgreSQL' : 'MySQL'} configs yet — create
          one on the DB Configs page first.
        </p>
      ) : (
        <select
          className={inputCls}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        >
          <option value="" disabled>
            Select a config…
          </option>
          {options.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name} ({c.host}:{c.port}/{c.database})
            </option>
          ))}
        </select>
      )}
    </Field>
  )
}
