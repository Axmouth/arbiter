import type { KvPair } from '../utils/keyvalue'

type KeyValueEditorProps = {
  pairs: KvPair[]
  onChange: (pairs: KvPair[]) => void
  keyPlaceholder?: string
  valuePlaceholder?: string
  addLabel?: string
}

/** A small repeated key/value row editor, used for env vars and HTTP headers. */
export function KeyValueEditor({
  pairs,
  onChange,
  keyPlaceholder = 'KEY',
  valuePlaceholder = 'value',
  addLabel = 'Add',
}: KeyValueEditorProps) {
  const update = (i: number, patch: Partial<KvPair>) => {
    onChange(pairs.map((p, idx) => (idx === i ? { ...p, ...patch } : p)))
  }
  const remove = (i: number) => onChange(pairs.filter((_, idx) => idx !== i))
  const add = () => onChange([...pairs, { key: '', value: '' }])

  return (
    <div className="space-y-2">
      {pairs.map((p, i) => (
        <div key={i} className="flex gap-2">
          <input
            type="text"
            value={p.key}
            placeholder={keyPlaceholder}
            autoComplete="off"
            onChange={(e) => update(i, { key: e.target.value })}
            className="flex-1 px-2 py-1 rounded font-mono text-sm bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
          />
          <input
            type="text"
            value={p.value}
            placeholder={valuePlaceholder}
            autoComplete="off"
            onChange={(e) => update(i, { value: e.target.value })}
            className="flex-1 px-2 py-1 rounded font-mono text-sm bg-(--bg-input) text-(--text-primary) border border-(--border-color)"
          />
          <button
            type="button"
            onClick={() => remove(i)}
            className="px-2 text-(--text-danger) hover:underline"
            aria-label="Remove"
          >
            ✕
          </button>
        </div>
      ))}
      <button
        type="button"
        onClick={add}
        className="text-sm text-(--text-accent) hover:underline"
      >
        + {addLabel}
      </button>
    </div>
  )
}
