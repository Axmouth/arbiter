import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useSecrets } from '../hooks/useSecrets'
import { useCreateSecret } from '../hooks/useCreateSecret'
import { deleteSecret } from '../api/secrets'
import { SlideOver } from '../components/SlideOver'
import { Button } from '../components/Button'
import { Table, THead, Th, TBody, Tr, Td } from '../components/Table'
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

      <Button variant="primary" onClick={() => setCreateOpen(true)}>
        New Secret
      </Button>

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}

      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {secrets &&
        (secrets.length === 0 ? (
          <div className="text-(--text-muted)">No secrets yet.</div>
        ) : (
          <Table>
            <THead>
              <Th>Name</Th>
              <Th>Key version</Th>
              <Th>Created</Th>
              <Th>Updated</Th>
              <Th align="right">Actions</Th>
            </THead>
            <TBody>
              {secrets.map((s) => (
                <Tr key={s.id}>
                  <Td className="font-mono">{s.name}</Td>
                  <Td>v{s.kekVersion}</Td>
                  <Td>{formatTime(s.createdAt)}</Td>
                  <Td>{formatTime(s.updatedAt)}</Td>
                  <Td align="right">
                    <Button
                      variant="ghost"
                      className="text-(--text-danger)"
                      onClick={() => {
                        if (confirm(`Delete secret "${s.name}"?`)) {
                          deleteMutation.mutate(s.id)
                        }
                      }}
                    >
                      Delete
                    </Button>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
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

  const createMutation = useCreateSecret()

  const canSubmit = name.trim().length > 0 && value.length > 0

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault()
        if (canSubmit) {
          createMutation.mutate(
            { name: name.trim(), value },
            { onSuccess: onDone }
          )
        }
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
        <Button
          type="submit"
          variant="primary"
          disabled={!canSubmit || createMutation.isPending}
        >
          {createMutation.isPending ? 'Saving…' : 'Save'}
        </Button>
        <Button type="button" variant="secondary" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  )
}
