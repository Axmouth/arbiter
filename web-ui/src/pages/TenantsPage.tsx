import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useTenants } from '../hooks/useTenants'
import { createTenant } from '../api/tenants'
import { useAuth } from '../auth/useAuth'
import { SlideOver } from '../components/SlideOver'
import { formatTime } from '../utils/time'

export function TenantsPage() {
  const { data: tenants, isLoading, error } = useTenants()
  const { state } = useAuth()
  const [createOpen, setCreateOpen] = useState(false)

  // Only a system admin (admin role, no tenant) may create tenants.
  const isSystemAdmin =
    state.status === 'authenticated' &&
    state.user.role === 'admin' &&
    state.user.tenantId == null

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold text-(--text-primary)">Tenants</h2>

      <p className="text-sm text-(--text-muted) max-w-2xl">
        Tenants isolate jobs, secrets, and configs. A system admin manages all
        tenants; a tenant admin sees only their own.
      </p>

      {isSystemAdmin && (
        <button
          onClick={() => setCreateOpen(true)}
          className="
            bg-(--bg-btn-primary) text-(--text-inverse)
            px-3 py-1.5 rounded
            hover:bg-(--bg-btn-primary-hover)
          "
        >
          New tenant
        </button>
      )}

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {tenants &&
        (tenants.length === 0 ? (
          <div className="text-(--text-muted)">No tenants.</div>
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
                  <th className="px-3 py-1.5 font-semibold">Name</th>
                  <th className="px-3 py-1.5 font-semibold">ID</th>
                  <th className="px-3 py-1.5 font-semibold">Created</th>
                </tr>
              </thead>

              <tbody className="divide-y divide-(--border-subtle)">
                {tenants.map((t) => (
                  <tr key={t.id} className="hover:bg-(--bg-row-hover)">
                    <td className="px-3 py-1.5">{t.name}</td>
                    <td className="px-3 py-1.5 font-mono text-(--text-muted)">
                      {t.id}
                    </td>
                    <td className="px-3 py-1.5">{formatTime(t.createdAt)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ))}

      <SlideOver
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="New tenant"
      >
        <TenantForm onDone={() => setCreateOpen(false)} />
      </SlideOver>
    </div>
  )
}

function TenantForm({ onDone }: { onDone: () => void }) {
  const [name, setName] = useState('')
  const qc = useQueryClient()

  const createMutation = useMutation({
    mutationFn: () => createTenant({ name: name.trim() }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['tenants'] })
      onDone()
    },
  })

  const canSubmit = name.trim().length > 0

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault()
        if (canSubmit) createMutation.mutate()
      }}
    >
      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Name</span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="acme"
          autoComplete="off"
          className="
            w-full px-3 py-1.5 rounded
            bg-(--bg-input) text-(--text-primary)
            border border-(--border-color)
          "
        />
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
            bg-(--bg-btn-primary) text-(--text-inverse)
            px-3 py-1.5 rounded
            hover:bg-(--bg-btn-primary-hover)
            disabled:opacity-50
          "
        >
          {createMutation.isPending ? 'Creating…' : 'Create'}
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
