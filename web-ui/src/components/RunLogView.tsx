import { useEffect, useRef, useState } from 'react'
import type { LogChunk } from '../backend-types'

/**
 * Render a run's captured output from append-only chunks. stderr is tinted. Auto-scrolls to
 * the bottom while the viewer is near the bottom (so live output follows without yanking the
 * view if the user has scrolled up to read). "Load earlier" pages backward; "Pop out" opens a
 * taller full-screen view. (Virtualized scrolling for very large logs is a follow-up; for now
 * the window is what has been paged in.)
 */
export function RunLogView({
  chunks,
  loadEarlier,
  loadingEarlier,
  hasEarlier,
  live,
}: {
  chunks: LogChunk[]
  loadEarlier: () => void
  loadingEarlier: boolean
  hasEarlier: boolean
  live: boolean
}) {
  const [popped, setPopped] = useState(false)

  const body = (tall: boolean) => (
    <LogBody
      chunks={chunks}
      loadEarlier={loadEarlier}
      loadingEarlier={loadingEarlier}
      hasEarlier={hasEarlier}
      tall={tall}
    />
  )

  return (
    <div>
      <div className="flex items-center justify-between mb-1">
        <h3 className="text-sm font-semibold text-(--text-primary)">
          Output {live && <span className="text-(--text-muted) font-normal">(live)</span>}
        </h3>
        <button
          onClick={() => setPopped(true)}
          className="text-xs text-(--text-accent) hover:underline"
        >
          Pop out
        </button>
      </div>
      {body(false)}

      {popped && (
        <div
          className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center p-6"
          onClick={() => setPopped(false)}
        >
          <div
            className="bg-(--bg-surface-alt) rounded-lg shadow-xl w-full max-w-5xl h-[85vh] flex flex-col p-4"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between mb-2">
              <h3 className="text-sm font-semibold text-(--text-primary)">Output</h3>
              <button
                onClick={() => setPopped(false)}
                className="text-xs text-(--text-accent) hover:underline"
              >
                Close
              </button>
            </div>
            <div className="flex-1 min-h-0">{body(true)}</div>
          </div>
        </div>
      )}
    </div>
  )
}

function LogBody({
  chunks,
  loadEarlier,
  loadingEarlier,
  hasEarlier,
  tall,
}: {
  chunks: LogChunk[]
  loadEarlier: () => void
  loadingEarlier: boolean
  hasEarlier: boolean
  tall: boolean
}) {
  const ref = useRef<HTMLDivElement>(null)
  // Follow the tail only when already near the bottom, so reading earlier output is not
  // interrupted by live appends.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80
    if (nearBottom) el.scrollTop = el.scrollHeight
  }, [chunks])

  return (
    <div
      ref={ref}
      className={`${tall ? 'h-full' : 'max-h-72'} overflow-auto rounded border bg-(--bg-code) border-(--border-color) p-3 text-sm`}
    >
      {hasEarlier && (
        <button
          onClick={loadEarlier}
          disabled={loadingEarlier}
          className="mb-2 text-xs text-(--text-accent) hover:underline disabled:opacity-50"
        >
          {loadingEarlier ? 'Loading…' : 'Load earlier'}
        </button>
      )}
      {chunks.length === 0 ? (
        <span className="text-(--text-muted) italic">&lt;no output&gt;</span>
      ) : (
        <pre className="whitespace-pre-wrap break-words">
          {chunks.map((c, i) => (
            <span
              key={`${c.seq}-${i}`}
              className={c.stream === 'stderr' ? 'text-(--text-error)' : 'text-(--text-code)'}
            >
              {c.content}
            </span>
          ))}
        </pre>
      )}
    </div>
  )
}
