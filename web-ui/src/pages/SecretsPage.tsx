import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useSecrets } from '../hooks/useSecrets'
import { createSecret, deleteSecret } from '../api/secrets'
import { SlideOver } from '../components/SlideOver'
import { formatTime } from '../utils/time'

export function SecretsPage() {
  const { data: secrets, isLoading, error } = useSecrets()
  const [createOpen, setCreateOpen] = useState(false)
  const qc = useQueryClient()

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteSecret(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['secrets'] }),
  })

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold text-(--text-primary)">Secrets</h2>

      <p className="text-sm text-(--text-muted) max-w-2xl">
        Secret values are write-only: they are encrypted on the server and never
        shown again. Reference one from a job env var or DB config as{' '}
        <code className="text-(--text-primary)">secret:&lt;name&gt;</code>.
      </p>

      <button
        onClick={() => setCreateOpen(true)}
        className="
          bg-(--bg-btn-primary) text-(--text-inverse) border border-(--border-color) text-[13px]
          px-3 py-1.5 rounded
          hover:bg-(--bg-btn-primary-hover)
        "
      >
        New Secret
      </button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {secrets &&
        (secrets.length === 0 ? (
          <div className="text-(--text-muted)">No secrets yet.</div>
        ) : (
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
                  <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)">Key version</th>
                  <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)">Created</th>
                  <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)">Updated</th>
                  <th className="px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted) text-right">Actions</th>
                </tr>
              </thead>

              <tbody className="divide-y divide-(--border-subtle)">
                {secrets.map((s) => (
                  <tr key={s.id} className="hover:bg-(--bg-row-hover)">
                    <td className="px-3 py-1.5 font-mono">{s.name}</td>
                    <td className="px-3 py-1.5">v{s.kekVersion}</td>
                    <td className="px-3 py-1.5">{formatTime(s.createdAt)}</td>
                    <td className="px-3 py-1.5">{formatTime(s.updatedAt)}</td>
                    <td className="px-3 py-1.5 text-right">
                      <button
                        onClick={() => {
                          if (confirm(`Delete secret "${s.name}"?`)) {
                            deleteMutation.mutate(s.id)
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
        title="New Secret"
      >
        <SecretForm onDone={() => setCreateOpen(false)} />
      </SlideOver>
    </div>
  )
}

function SecretForm({ onDone }: { onDone: () => void }) {
  const [name, setName] = useState('')
  const [value, setValue] = useState('')
  const qc = useQueryClient()

  const createMutation = useMutation({
    mutationFn: () => createSecret({ name: name.trim(), value }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['secrets'] })
      onDone()
    },
  })

  const canSubmit = name.trim().length > 0 && value.length > 0

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault()
        if (canSubmit) createMutation.mutate()
      }}
    >
      <p className="text-sm text-(--text-muted)">
        Creating a secret with an existing name replaces its value.
      </p>

      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Name</span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="db-password"
          autoComplete="off"
          className="
            w-full px-3 py-1.5 rounded font-mono
            bg-(--bg-input) text-(--text-primary)
            border border-(--border-color)
          "
        />
      </label>

      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Value</span>
        <input
          type="password"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          autoComplete="new-password"
          className="
            w-full px-3 py-1.5 rounded
            bg-(--bg-input) text-(--text-primary)
            border border-(--border-color)
          "
        />
        <span className="text-xs text-(--text-muted)">
          Stored encrypted; it cannot be viewed later, only replaced.
        </span>
      </label>

      {createMutation.error && (
        <div className="text-(--text-danger) text-sm">
          {String(createMutation.error)}
        </div>
      )}

      <div className="flex gap-3 pt-2">
        <button
          type="submit"
          disabled={!canSubmit || createMutation.isPending}
          className="
            bg-(--bg-btn-primary) text-(--text-inverse) border border-(--border-color) text-[13px]
            px-3 py-1.5 rounded
            hover:bg-(--bg-btn-primary-hover)
            disabled:opacity-50
          "
        >
          {createMutation.isPending ? 'Saving…' : 'Save'}
        </button>
        <button
          type="button"
          onClick={onDone}
          className="px-3 py-1.5 rounded border border-(--border-color) text-(--text-secondary)"
        >
          Cancel
        </button>
      </div>
    </form>
  )
}
