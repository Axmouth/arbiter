import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useUsers } from '../hooks/useUsers'
import { useTenants } from '../hooks/useTenants'
import { useAuth } from '../auth/useAuth'
import { createUser, deleteUser, updateUser } from '../api/users'
import { SlideOver } from '../components/SlideOver'
import { Button } from '../components/Button'
import { Table, THead, Th, TBody, Tr, Td } from '../components/Table'
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

      <Button variant="primary" onClick={() => setCreateOpen(true)}>
        New user
      </Button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {users && (
        <Table>
          <THead>
            <Th>Username</Th>
            <Th>Role</Th>
            <Th>Tenant</Th>
            <Th>Created</Th>
            <Th align="right">Actions</Th>
          </THead>
          <TBody>
            {users.map((u) => (
              <Tr key={u.id} onClick={() => setEditing(u)}>
                <Td>{u.username}</Td>
                <Td>{u.role}</Td>
                <Td>{tenantName(u.tenantId)}</Td>
                <Td>{formatTime(u.createdAt)}</Td>
                <Td align="right">
                  <Button
                    variant="ghost"
                    className="text-(--text-danger)"
                    disabled={u.id === currentUserId}
                    onClick={(e) => {
                      e.stopPropagation()
                      if (confirm(`Delete user "${u.username}"?`)) {
                        deleteMutation.mutate(u.id)
                      }
                    }}
                    title={
                      u.id === currentUserId ? 'You cannot delete yourself' : undefined
                    }
                  >
                    Delete
                  </Button>
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
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
      <Button type="submit" variant="primary" disabled={!canSubmit || pending}>
        {pending ? 'Saving…' : submitLabel}
      </Button>
      <Button type="button" variant="secondary" onClick={onCancel}>
        Cancel
      </Button>
    </div>
  )
}
