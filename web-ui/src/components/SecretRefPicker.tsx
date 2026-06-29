import { useState } from 'react'
import { useSecrets } from '../hooks/useSecrets'
import { useCreateSecret } from '../hooks/useCreateSecret'
import { Button } from './Button'

type Props = {
  // The current `secret:<name>` reference, or '' when none is chosen.
  value: string
  onChange: (ref: string) => void
}

// Sentinel option that switches the picker into inline-create mode rather than
// selecting an existing reference.
const NEW = '__new__'

const selectCls =
  'w-full px-3 py-1.5 rounded font-mono bg-(--bg-input) text-(--text-primary) border border-(--border-color)'
const inputCls =
  'w-full px-3 py-1.5 rounded bg-(--bg-input) text-(--text-primary) border border-(--border-color)'

// Picks a secret reference, with an inline "+ New secret" path so a user does not
// have to leave for the Secrets page and come back. Creating one writes through
// the shared create mutation and immediately selects the new reference.
export function SecretRefPicker({ value, onChange }: Props) {
  const { data: secrets } = useSecrets()
  const create = useCreateSecret()

  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')
  const [newValue, setNewValue] = useState('')

  const refs = (secrets ?? []).map((s) => `secret:${s.name}`)
  // Keep the current value selectable even if its secret was since removed.
  const options = Array.from(
    new Set([value, ...refs].filter((r) => r.length > 0))
  )

  const canCreate = newName.trim().length > 0 && newValue.length > 0

  function submitNew() {
    if (!canCreate) return
    const name = newName.trim()
    create.mutate(
      { name, value: newValue },
      {
        onSuccess: () => {
          onChange(`secret:${name}`)
          setCreating(false)
          setNewName('')
          setNewValue('')
        },
      }
    )
  }

  if (creating) {
    return (
      <div className="space-y-2 rounded border border-(--border-subtle) p-3">
        <span className="text-xs text-(--text-muted)">New secret</span>
        <input
          type="text"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="db-password"
          autoComplete="off"
          className={`${inputCls} font-mono`}
        />
        <input
          type="password"
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
          placeholder="value"
          autoComplete="new-password"
          // Enter inside an inline field would submit the surrounding form, so
          // create on Enter and keep the outer form untouched.
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              submitNew()
            }
          }}
          className={inputCls}
        />
        {create.error && (
          <div className="text-(--text-danger) text-sm">
            {String(create.error)}
          </div>
        )}
        <div className="flex gap-2">
          <Button
            type="button"
            variant="primary"
            disabled={!canCreate || create.isPending}
            onClick={submitNew}
          >
            {create.isPending ? 'Creating…' : 'Create & use'}
          </Button>
          <Button
            type="button"
            variant="secondary"
            onClick={() => setCreating(false)}
          >
            Cancel
          </Button>
        </div>
        <span className="text-xs text-(--text-muted)">
          Stored encrypted; it cannot be viewed later, only replaced.
        </span>
      </div>
    )
  }

  return (
    <select
      value={value}
      onChange={(e) => {
        if (e.target.value === NEW) {
          setCreating(true)
        } else {
          onChange(e.target.value)
        }
      }}
      className={selectCls}
    >
      <option value="" disabled>
        Select a secret…
      </option>
      {options.map((ref) => (
        <option key={ref} value={ref}>
          {ref}
        </option>
      ))}
      <option value={NEW}>+ New secret…</option>
    </select>
  )
}
