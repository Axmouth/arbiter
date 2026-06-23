import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useDbConfigs } from '../hooks/useDbConfigs'
import { useSecrets } from '../hooks/useSecrets'
import { createDbConfig, deleteDbConfig, updateDbConfig } from '../api/configs'
import { SlideOver } from '../components/SlideOver'
import type { DbEngine, SharedDbConfig } from '../backend-types'

const ENGINE_LABEL: Record<DbEngine, string> = {
  pgSql: 'PostgreSQL',
  mySql: 'MySQL',
}

const DEFAULT_PORT: Record<DbEngine, number> = {
  pgSql: 5432,
  mySql: 3306,
}

export function DbConfigsPage() {
  const { data: configs, isLoading, error } = useDbConfigs()
  const [createOpen, setCreateOpen] = useState(false)
  const [editing, setEditing] = useState<SharedDbConfig | null>(null)
  const qc = useQueryClient()

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteDbConfig(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['db-configs'] }),
  })

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-semibold text-(--text-primary)">
        Database configs
      </h2>

      <p className="text-sm text-(--text-muted) max-w-2xl">
        Shared connection settings for the PostgreSQL and MySQL runners. The
        password is a secret reference, resolved at execution — never stored in
        plaintext.
      </p>

      <button
        onClick={() => setCreateOpen(true)}
        className="
          bg-(--bg-btn-primary) text-(--text-inverse)
          px-4 py-2 rounded
          hover:bg-(--bg-btn-primary-hover)
        "
      >
        New config
      </button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {configs &&
        (configs.length === 0 ? (
          <div className="text-(--text-muted)">No configs yet.</div>
        ) : (
          <div
            className="
              rounded-lg shadow border border-(--border-color)
              overflow-hidden bg-(--bg-surface-alt)
            "
          >
            <table className="w-full text-left">
              <thead className="bg-(--bg-header) text-(--text-primary) border-b border-(--border-subtle)">
                <tr>
                  <th className="px-4 py-2 font-semibold">Name</th>
                  <th className="px-4 py-2 font-semibold">Engine</th>
                  <th className="px-4 py-2 font-semibold">Host</th>
                  <th className="px-4 py-2 font-semibold">Database</th>
                  <th className="px-4 py-2 font-semibold">User</th>
                  <th className="px-4 py-2 font-semibold">Password</th>
                  <th className="px-4 py-2 font-semibold text-right">Actions</th>
                </tr>
              </thead>

              <tbody className="divide-y divide-(--border-subtle)">
                {configs.map((c) => (
                  <tr
                    key={c.id}
                    className="hover:bg-(--bg-row-hover) cursor-pointer"
                    onClick={() => setEditing(c)}
                  >
                    <td className="px-4 py-2">{c.name}</td>
                    <td className="px-4 py-2">{ENGINE_LABEL[c.engine]}</td>
                    <td className="px-4 py-2 font-mono">
                      {c.host}:{c.port}
                    </td>
                    <td className="px-4 py-2">{c.database}</td>
                    <td className="px-4 py-2">{c.username}</td>
                    <td className="px-4 py-2 font-mono text-(--text-muted)">
                      {c.passwordSecret}
                    </td>
                    <td className="px-4 py-2 text-right">
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          if (confirm(`Delete config "${c.name}"?`)) {
                            deleteMutation.mutate(c.id)
                          }
                        }}
                        className="text-(--text-danger) hover:underline"
                      >
                        Delete
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ))}

      <SlideOver
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="New database config"
      >
        <DbConfigForm mode="create" onDone={() => setCreateOpen(false)} />
      </SlideOver>

      <SlideOver
        open={editing !== null}
        onClose={() => setEditing(null)}
        title={editing ? `Edit ${editing.name}` : ''}
      >
        {editing && (
          <DbConfigForm
            mode="edit"
            initial={editing}
            onDone={() => setEditing(null)}
          />
        )}
      </SlideOver>
    </div>
  )
}

type FormProps =
  | { mode: 'create'; initial?: undefined; onDone: () => void }
  | { mode: 'edit'; initial: SharedDbConfig; onDone: () => void }

function DbConfigForm({ mode, initial, onDone }: FormProps) {
  const qc = useQueryClient()
  const { data: secrets } = useSecrets()

  const [engine, setEngine] = useState<DbEngine>(initial?.engine ?? 'pgSql')
  const [name, setName] = useState(initial?.name ?? '')
  const [host, setHost] = useState(initial?.host ?? '')
  const [port, setPort] = useState<number>(initial?.port ?? DEFAULT_PORT.pgSql)
  const [username, setUsername] = useState(initial?.username ?? '')
  const [database, setDatabase] = useState(initial?.database ?? '')
  const [passwordSecret, setPasswordSecret] = useState(
    initial?.passwordSecret ?? ''
  )

  // Offer existing secrets as references; keep the current value selectable when
  // editing even if its secret was since removed.
  const secretRefs = (secrets ?? []).map((s) => `secret:${s.name}`)
  const options = Array.from(
    new Set([passwordSecret, ...secretRefs].filter((r) => r.length > 0))
  )

  const mutation = useMutation({
    mutationFn: () => {
      if (mode === 'edit') {
        return updateDbConfig(initial.id, {
          name,
          host,
          port,
          username,
          passwordSecret,
          database,
        })
      }
      return createDbConfig({
        engine,
        name,
        host,
        port,
        username,
        passwordSecret,
        database,
      })
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['db-configs'] })
      onDone()
    },
  })

  const canSubmit =
    name.trim() !== '' &&
    host.trim() !== '' &&
    username.trim() !== '' &&
    database.trim() !== '' &&
    passwordSecret.trim() !== ''

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault()
        if (canSubmit) mutation.mutate()
      }}
    >
      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Engine</span>
        <select
          value={engine}
          disabled={mode === 'edit'}
          onChange={(e) => {
            const next = e.target.value as DbEngine
            setEngine(next)
            // Snap the port to the new engine's default if it was the old default.
            setPort((p) =>
              p === DEFAULT_PORT.pgSql || p === DEFAULT_PORT.mySql
                ? DEFAULT_PORT[next]
                : p
            )
          }}
          className="
            w-full px-3 py-2 rounded
            bg-(--bg-input) text-(--text-primary)
            border border-(--border-color)
            disabled:opacity-60
          "
        >
          <option value="pgSql">{ENGINE_LABEL.pgSql}</option>
          <option value="mySql">{ENGINE_LABEL.mySql}</option>
        </select>
        {mode === 'edit' && (
          <span className="text-xs text-(--text-muted)">
            Engine is fixed once created.
          </span>
        )}
      </label>

      <Field label="Name" value={name} onChange={setName} placeholder="primary" />
      <Field label="Host" value={host} onChange={setHost} placeholder="db.internal" />

      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Port</span>
        <input
          type="number"
          value={port}
          min={1}
          max={65535}
          onChange={(e) => setPort(Number(e.target.value))}
          className="
            w-full px-3 py-2 rounded
            bg-(--bg-input) text-(--text-primary)
            border border-(--border-color)
          "
        />
      </label>

      <Field label="Username" value={username} onChange={setUsername} />
      <Field label="Database" value={database} onChange={setDatabase} />

      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Password secret</span>
        {options.length === 0 ? (
          <p className="text-xs text-(--text-danger)">
            No secrets yet — create one on the Secrets page first.
          </p>
        ) : (
          <select
            value={passwordSecret}
            onChange={(e) => setPasswordSecret(e.target.value)}
            className="
              w-full px-3 py-2 rounded font-mono
              bg-(--bg-input) text-(--text-primary)
              border border-(--border-color)
            "
          >
            <option value="" disabled>
              Select a secret…
            </option>
            {options.map((ref) => (
              <option key={ref} value={ref}>
                {ref}
              </option>
            ))}
          </select>
        )}
        <span className="text-xs text-(--text-muted)">
          The connection password is resolved from this secret at execution.
        </span>
      </label>

      {mutation.error && (
        <div className="text-(--text-danger) text-sm">
          {String(mutation.error)}
        </div>
      )}

      <div className="flex gap-3 pt-2">
        <button
          type="submit"
          disabled={!canSubmit || mutation.isPending}
          className="
            bg-(--bg-btn-primary) text-(--text-inverse)
            px-4 py-2 rounded
            hover:bg-(--bg-btn-primary-hover)
            disabled:opacity-50
          "
        >
          {mutation.isPending ? 'Saving…' : 'Save'}
        </button>
        <button
          type="button"
          onClick={onDone}
          className="px-4 py-2 rounded border border-(--border-color) text-(--text-secondary)"
        >
          Cancel
        </button>
      </div>
    </form>
  )
}

function Field({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string
  value: string
  onChange: (v: string) => void
  placeholder?: string
}) {
  return (
    <label className="block space-y-1">
      <span className="text-sm text-(--text-secondary)">{label}</span>
      <input
        type="text"
        value={value}
        placeholder={placeholder}
        autoComplete="off"
        onChange={(e) => onChange(e.target.value)}
        className="
          w-full px-3 py-2 rounded
          bg-(--bg-input) text-(--text-primary)
          border border-(--border-color)
        "
      />
    </label>
  )
}
