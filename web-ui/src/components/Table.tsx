import type { ReactNode } from 'react'

/**
 * Shared table primitives so every list renders with the same shell, header treatment, row
 * dividers, and hover. Styling lives here once; pages compose Table > THead/Th + TBody/Tr/Td
 * and put their own cells inside. `align="right"` right-aligns a header or cell.
 */
export function Table({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-lg border border-(--border-color) overflow-hidden bg-(--bg-surface-alt)">
      <table className="w-full text-left">{children}</table>
    </div>
  )
}

/** Header row. Pass <Th> children directly; this renders the <tr>. */
export function THead({ children }: { children: ReactNode }) {
  return (
    <thead className="bg-(--bg-header) border-b border-(--border-subtle)">
      <tr>{children}</tr>
    </thead>
  )
}

export function Th({
  children,
  align,
}: {
  children?: ReactNode
  align?: 'right'
}) {
  return (
    <th
      className={`px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-(--text-muted)${
        align === 'right' ? ' text-right' : ''
      }`}
    >
      {children}
    </th>
  )
}

export function TBody({ children }: { children: ReactNode }) {
  return <tbody className="divide-y divide-(--border-subtle)">{children}</tbody>
}

export function Tr({
  children,
  onClick,
  className = '',
}: {
  children: ReactNode
  onClick?: () => void
  className?: string
}) {
  return (
    <tr
      onClick={onClick}
      className={`hover:bg-(--bg-row-hover)${onClick ? ' cursor-pointer' : ''}${
        className ? ' ' + className : ''
      }`}
    >
      {children}
    </tr>
  )
}

export function Td({
  children,
  align,
  className = '',
}: {
  children?: ReactNode
  align?: 'right'
  className?: string
}) {
  return (
    <td
      className={`px-3 py-1.5${align === 'right' ? ' text-right' : ''}${
        className ? ' ' + className : ''
      }`}
    >
      {children}
    </td>
  )
}
