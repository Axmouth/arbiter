import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useUsers } from '../hooks/useUsers'
import { useTenants } from '../hooks/useTenants'
import { useAuth } from '../auth/useAuth'
import { createUser, deleteUser, updateUser } from '../api/users'
import { SlideOver } from '../components/SlideOver'
import { formatTime } from '../utils/time'
import type { User, UserRole } from '../backend-types'

const ROLES: UserRole[] = ['admin', 'operator', 'viewer']

export function UsersPage() {
  const { data: users, isLoading, error } = useUsers()
  const { data: tenants } = useTenants()
  const { state } = useAuth()
  const [createOpen, setCreateOpen] = useState(false)
  const [editing, setEditing] = useState<User | null>(null)
  const qc = useQueryClient()

  const currentUserId =
    state.status === 'authenticated' ? state.user.id : undefined

  const tenantName = (id: string | null) => {
    if (id == null) return 'System'
    return tenants?.find((t) => t.id === id)?.name ?? id
  }

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteUser(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['users'] }),
  })

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold text-(--text-primary)">Users</h2>

      <button
        onClick={() => setCreateOpen(true)}
        className="
          bg-(--bg-btn-primary) text-(--text-inverse)
          px-3 py-1.5 rounded
          hover:bg-(--bg-btn-primary-hover)
        "
      >
        New user
      </button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {users && (
        <div
          className="
            rounded-lg border border-(--border-color)
            overflow-hidden bg-(--bg-surface-alt)
          "
        >
          <table className="w-full text-left">
            <thead className="bg-(--bg-header) text-(--text-primary) border-b border-(--border-subtle)">
              <tr>
                <th className="px-3 py-1.5 font-semibold">Username</th>
                <th className="px-3 py-1.5 font-semibold">Role</th>
                <th className="px-3 py-1.5 font-semibold">Tenant</th>
                <th className="px-3 py-1.5 font-semibold">Created</th>
                <th className="px-3 py-1.5 font-semibold text-right">Actions</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-(--border-subtle)">
              {users.map((u) => (
                <tr
                  key={u.id}
                  className="hover:bg-(--bg-row-hover) cursor-pointer"
                  onClick={() => setEditing(u)}
                >
                  <td className="px-3 py-1.5">{u.username}</td>
                  <td className="px-3 py-1.5">{u.role}</td>
                  <td className="px-3 py-1.5">{tenantName(u.tenantId)}</td>
                  <td className="px-3 py-1.5">{formatTime(u.createdAt)}</td>
                  <td className="px-3 py-1.5 text-right">
                    <button
                      disabled={u.id === currentUserId}
                      onClick={(e) => {
                        e.stopPropagation()
                        if (confirm(`Delete user "${u.username}"?`)) {
                          deleteMutation.mutate(u.id)
                        }
                      }}
                      className="text-(--text-danger) hover:underline disabled:opacity-40 disabled:no-underline"
                      title={
                        u.id === currentUserId
                          ? 'You cannot delete yourself'
                          : undefined
                      }
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <SlideOver
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="New user"
      >
        <CreateUserForm onDone={() => setCreateOpen(false)} />
      </SlideOver>

      <SlideOver
        open={editing !== null}
        onClose={() => setEditing(null)}
        title={editing ? `Edit ${editing.username}` : ''}
      >
        {editing && (
          <EditUserForm user={editing} onDone={() => setEditing(null)} />
        )}
      </SlideOver>
    </div>
  )
}

function CreateUserForm({ onDone }: { onDone: () => void }) {
  const qc = useQueryClient()
  const { data: tenants } = useTenants()
  const { state } = useAuth()
  const isSystemAdmin =
    state.status === 'authenticated' &&
    state.user.role === 'admin' &&
    state.user.tenantId == null

  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [role, setRole] = useState<UserRole>('viewer')
  const [tenantId, setTenantId] = useState<string>('') // '' = System

  const mutation = useMutation({
    mutationFn: () =>
      createUser({
        username: username.trim(),
        password,
        role,
        tenantId: tenantId === '' ? null : tenantId,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['users'] })
      onDone()
    },
  })

  const canSubmit = username.trim() !== '' && password !== ''

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault()
        if (canSubmit) mutation.mutate()
      }}
    >
      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Username</span>
        <input
          type="text"
          value={username}
          autoComplete="off"
          onChange={(e) => setUsername(e.target.value)}
          className="w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
        />
      </label>

      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Password</span>
        <input
          type="password"
          value={password}
          autoComplete="new-password"
          onChange={(e) => setPassword(e.target.value)}
          className="w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
        />
      </label>

      <RoleSelect role={role} onChange={setRole} />

      {isSystemAdmin && (
        <label className="block space-y-1">
          <span className="text-sm text-(--text-secondary)">Tenant</span>
          <select
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            className="w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
          >
            <option value="">System (no tenant)</option>
            {(tenants ?? []).map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
        </label>
      )}

      {mutation.error && (
        <div className="text-(--text-danger) text-sm">
          {String(mutation.error)}
        </div>
      )}

      <FormButtons pending={mutation.isPending} canSubmit={canSubmit} onCancel={onDone} submitLabel="Create" />
    </form>
  )
}

function EditUserForm({ user, onDone }: { user: User; onDone: () => void }) {
  const qc = useQueryClient()
  const [username, setUsername] = useState(user.username)
  const [password, setPassword] = useState('')
  const [role, setRole] = useState<UserRole>(user.role)

  const mutation = useMutation({
    mutationFn: () =>
      updateUser(user.id, {
        username: username.trim() === user.username ? null : username.trim(),
        password: password === '' ? null : password,
        role: role === user.role ? null : role,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['users'] })
      onDone()
    },
  })

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault()
        mutation.mutate()
      }}
    >
      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">Username</span>
        <input
          type="text"
          value={username}
          autoComplete="off"
          onChange={(e) => setUsername(e.target.value)}
          className="w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
        />
      </label>

      <label className="block space-y-1">
        <span className="text-sm text-(--text-secondary)">
          New password{' '}
          <span className="text-(--text-muted)">(leave blank to keep)</span>
        </span>
        <input
          type="password"
          value={password}
          autoComplete="new-password"
          onChange={(e) => setPassword(e.target.value)}
          className="w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
        />
      </label>

      <RoleSelect role={role} onChange={setRole} />

      <p className="text-xs text-(--text-muted)">
        A user's tenant is fixed after creation.
      </p>

      {mutation.error && (
        <div className="text-(--text-danger) text-sm">
          {String(mutation.error)}
        </div>
      )}

      <FormButtons pending={mutation.isPending} canSubmit onCancel={onDone} submitLabel="Save" />
    </form>
  )
}

function RoleSelect({
  role,
  onChange,
}: {
  role: UserRole
  onChange: (r: UserRole) => void
}) {
  return (
    <label className="block space-y-1">
      <span className="text-sm text-(--text-secondary)">Role</span>
      <select
        value={role}
        onChange={(e) => onChange(e.target.value as UserRole)}
        className="w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
      >
        {ROLES.map((r) => (
          <option key={r} value={r}>
            {r}
          </option>
        ))}
      </select>
    </label>
  )
}

function FormButtons({
  pending,
  canSubmit,
  onCancel,
  submitLabel,
}: {
  pending: boolean
  canSubmit: boolean
  onCancel: () => void
  submitLabel: string
}) {
  return (
    <div className="flex gap-3 pt-2">
      <button
        type="submit"
        disabled={!canSubmit || pending}
        className="bg-(--bg-btn-primary) text-(--text-inverse) px-3 py-1.5 rounded hover:bg-(--bg-btn-primary-hover) disabled:opacity-50"
      >
        {pending ? 'Saving…' : submitLabel}
      </button>
      <button
        type="button"
        onClick={onCancel}
        className="px-3 py-1.5 rounded border border-(--border-color) text-(--text-secondary)"
      >
        Cancel
      </button>
    </div>
  )
}
