import type { ReactNode } from 'react'

export type BadgeTone =
  | 'success'
  | 'error'
  | 'warning'
  | 'neutral'
  | 'running'
  | 'info'

const toneClass: Record<BadgeTone, string> = {
  success: 'bg-(--bg-success) text-(--text-success)',
  error: 'bg-(--bg-error) text-(--text-error)',
  warning: 'bg-(--bg-warning) text-(--text-warning)',
  neutral: 'bg-(--bg-neutral) text-(--text-neutral)',
  running: 'bg-(--bg-running) text-(--text-running)',
  info: 'bg-(--bg-neutral) text-(--text-info)',
}

/**
 * Small status chip. One source of truth for the pill shape and tone -> color mapping; the
 * caller picks a semantic `tone`. Extra classes (e.g. a running animation) compose on top.
 */
export function Badge({
  tone = 'neutral',
  className = '',
  children,
}: {
  tone?: BadgeTone
  className?: string
  children: ReactNode
}) {
  return (
    <span className={`inline-block px-2 py-1 rounded text-xs ${toneClass[tone]} ${className}`.trim()}>
      {children}
    </span>
  )
}
