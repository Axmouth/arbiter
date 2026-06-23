export type KvPair = { key: string; value: string }

/** Convert editor rows to a record, dropping blank keys; `null` when empty. */
export function pairsToRecord(pairs: KvPair[]): Record<string, string> | null {
  const out: Record<string, string> = {}
  for (const p of pairs) {
    const k = p.key.trim()
    if (k !== '') out[k] = p.value
  }
  return Object.keys(out).length > 0 ? out : null
}

export function recordToPairs(
  rec: Record<string, string | undefined> | null | undefined
): KvPair[] {
  if (!rec) return []
  return Object.entries(rec).map(([key, value]) => ({ key, value: value ?? '' }))
}
