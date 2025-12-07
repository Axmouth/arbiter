import { useEffect, useRef, useState } from 'react'

export type DropdownItem<T> = {
  value: T
  label: string
}

interface SearchableDropdownProps<T> {
  label: string
  items: DropdownItem<T>[]
  value: T | undefined
  onChange: (v: T | undefined) => void
  placeholder?: string
  className?: string
}

export function SearchableDropdown<T extends string | number>({
  label,
  items,
  value,
  onChange,
  placeholder = 'Select…',
  className = '',
}: SearchableDropdownProps<T>) {
  const [open, setOpen] = useState(false)
  const [search, setSearch] = useState('')
  const ref = useRef<HTMLDivElement>(null)

  // Close when clicking outside
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [])

  const selectedItem = items.find((i) => i.value === value)
  const filtered = items.filter((item) =>
    item.label.toLowerCase().includes(search.toLowerCase())
  )

  return (
    <div className={`relative ${className}`} ref={ref}>
      <label className="block text-sm font-medium mb-1 text-(--text-primary)">
        {label}
      </label>
      <button
        type="button"
        onClick={() => {
          setSearch('')
          setOpen((o) => !o)
        }}
        className="
          border border-(--border-color) rounded px-2 py-1 w-full text-left
          bg-(--bg-app) text-(--text-primary)
        "
      >
        {selectedItem?.label ?? placeholder}
      </button>

      {open && (
        <div
          className="
            absolute left-0 mt-1 z-20
            bg-(--bg-popover) border border-(--border-color) rounded shadow
            w-max min-w-full max-w-lg
          "
        >
          <input
            autoFocus
            type="text"
            placeholder="Search…"
            className="
              px-2 py-1 border-b border-(--border-subtle) w-full
              bg-(--bg-popover) text-(--text-primary)
              placeholder-(--text-placeholder)
            "
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />

          <div className="max-h-60 overflow-auto">
            {filtered.length > 0 ? (
              filtered.map((item) => (
                <div
                  key={String(item.value)}
                  onClick={() => {
                    onChange(item.value)
                    setOpen(false)
                  }}
                  className="
                    px-2 py-2 cursor-pointer whitespace-nowrap
                    hover:bg-(--bg-popover-hover)
                    text-(--text-primary)
                  "
                >
                  {item.label}
                </div>
              ))
            ) : (
              <div className="px-2 py-2 text-(--text-muted) text-sm">
                No results
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
