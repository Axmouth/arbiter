import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNodeKeys } from '../hooks/useNodeKeys'
import { approveNode, revokeNode, evictNode, rotateKek, fetchRotationStatus } from '../api/nodes'
import type { RotateKekResponse } from '../backend-types'
import { Button } from '../components/Button'
import { Table, THead, Th, TBody, Tr, Td } from '../components/Table'
import { Badge } from '../components/Badge'
import { formatTime } from '../utils/time'

/// Watch rotation progress over Server-Sent Events. The browser EventSource sends the
/// session cookie, so no extra auth wiring is needed. The stream closes itself once no
/// rotation is in flight; bumping `watchKey` reopens it after a fresh rotate.
function useRotationStream(watchKey: number): { status: RotateKekResponse | null; done: boolean } {
  const [status, setStatus] = useState<RotateKekResponse | null>(null)
  // Track which watch the rotation finished on, so `done` resets for free on a new watchKey
  // (no synchronous setState in the effect).
  const [doneKey, setDoneKey] = useState<number | null>(null)

  useEffect(() => {
    const es = new EventSource('/api/v1/secrets/rotation/stream', { withCredentials: true })
    let sawActive = false
    es.onmessage = (e) => {
      const s = JSON.parse(e.data) as RotateKekResponse
      if (s.phase === 'idle') {
        if (sawActive) setDoneKey(watchKey)
        es.close()
        return
      }
      sawActive = true
      setStatus(s)
    }
    es.onerror = () => es.close()
    return () => es.close()
  }, [watchKey])

  return { status, done: doneKey === watchKey }
}

export function NodeKeysPage() {
  const { data: keys, isLoading, error } = useNodeKeys()
  const qc = useQueryClient()
  const [watchKey, setWatchKey] = useState(0)
  const { status: rotation, done: rotationDone } = useRotationStream(watchKey)
  // Current KEK version (the key all secrets are wrapped under), distinct from each node's
  // identity key version shown in the table.
  const { data: rotationStatus } = useQuery({
    queryKey: ['rotation-status'],
    queryFn: fetchRotationStatus,
  })

  const approveMutation = useMutation({
    mutationFn: (id: string) => approveNode(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['node-keys'] }),
  })
  const revokeMutation = useMutation({
    mutationFn: (id: string) => revokeNode(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['node-keys'] }),
  })
  const evictMutation = useMutation({
    mutationFn: (id: string) => evictNode(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['node-keys'] }),
  })
  const rotateMutation = useMutation({
    mutationFn: () => rotateKek(),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['node-keys'] })
      qc.invalidateQueries({ queryKey: ['rotation-status'] })
      // (Re)open the progress stream to watch the rotation through to completion.
      setWatchKey((k) => k + 1)
    },
  })

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-xl font-semibold text-(--text-primary)">Keyholders</h2>
          {rotationStatus?.activeVersion != null && (
            <p className="text-sm text-(--text-muted) mt-0.5">
              Current KEK: <span className="font-mono">v{rotationStatus.activeVersion}</span>
            </p>
          )}
        </div>
        <Button
          variant="primary"
          disabled={rotateMutation.isPending}
          onClick={() => {
            if (
              confirm(
                'Rotate the KEK? This re-encrypts every secret under a new key and locks ' +
                  'out any revoked node. It cannot be undone.',
              )
            ) {
              rotateMutation.mutate()
            }
          }}
        >
          {rotateMutation.isPending ? 'Rotating…' : 'Rotate KEK'}
        </Button>
      </div>

      <p className="text-sm text-(--text-muted) max-w-2xl">
        Nodes that may hold the encryption key (KEK) and therefore read/write secrets.
        A joining node registers as <strong>pending</strong>; approve it only after
        verifying its key fingerprint. Approving lets a key-holding node seal the KEK to it.
        Revoking a node stops future sealing; <strong>rotate the KEK</strong> afterward to
        re-encrypt secrets and fully lock the revoked node out. Evict a permanently-dead node
        so it cannot stall a rotation.
      </p>

      <RotationProgress
        rotation={rotation}
        result={rotateMutation.data ?? null}
        done={rotationDone}
      />

      {rotateMutation.isError && (
        <div className="text-(--text-danger) text-sm">{String(rotateMutation.error)}</div>
      )}

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}
      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {keys &&
        (keys.length === 0 ? (
          <div className="text-(--text-muted)">No node keys registered.</div>
        ) : (
          <Table>
            <THead>
              <Th>Node</Th>
              <Th>Identity key</Th>
              <Th>Fingerprint</Th>
              <Th>Status</Th>
              <Th>Approved</Th>
              <Th align="right">Actions</Th>
            </THead>
            <TBody>
              {keys.map((k) => (
                <Tr key={`${k.nodeId}-${k.keyVersion}`}>
                  <Td className="font-mono text-xs">{k.nodeId}</Td>
                  <Td>v{k.keyVersion}</Td>
                  <Td className="font-mono text-xs text-(--text-muted)">
                    {k.publicKey.slice(0, 16)}…
                  </Td>
                  <Td>
                    <StatusBadge status={k.status} />
                  </Td>
                  <Td>{formatTime(k.approvedAt)}</Td>
                  <Td align="right" className="space-x-3">
                    {k.status === 'approved' ? (
                        <Button
                          variant="ghost"
                          className="text-(--text-danger)"
                          onClick={() => {
                            if (confirm('Revoke this keyholder?')) {
                              revokeMutation.mutate(k.nodeId)
                            }
                          }}
                        >
                          Revoke
                        </Button>
                      ) : (
                        <Button
                          variant="ghost"
                          className="text-(--text-accent)"
                          onClick={() => approveMutation.mutate(k.nodeId)}
                        >
                          Approve
                        </Button>
                      )}
                      {k.status !== 'evicted' && (
                        <Button
                          variant="ghost"
                          className="text-(--text-muted)"
                          onClick={() => {
                            if (confirm('Evict this node? Use this only for a node that is gone for good.')) {
                              evictMutation.mutate(k.nodeId)
                            }
                          }}
                        >
                          Evict
                        </Button>
                      )}
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
        ))}
    </div>
  )
}

function RotationProgress({
  rotation,
  result,
  done,
}: {
  rotation: RotateKekResponse | null
  result: RotateKekResponse | null
  done: boolean
}) {
  // Prefer the live stream while a rotation is in flight; fall back to the rotate call's own
  // result so a single-node rotation (which completes inside the POST, before the stream
  // connects) still shows feedback.
  const snap = rotation ?? result
  if (!snap) return null

  if (done || snap.phase === 'done') {
    return (
      <div className="text-(--text-success) text-sm">
        KEK rotated to v{snap.targetVersion}. All {snap.secretsTotal} secret(s) re-encrypted.
      </div>
    )
  }

  const phaseLabel =
    snap.phase === 'distributing'
      ? 'Distributing the new key to nodes'
      : snap.phase === 'rewrapping'
        ? 'Re-encrypting secrets'
        : snap.phase

  return (
    <div className="rounded-lg border border-(--border-color) bg-(--bg-surface-alt) p-4 space-y-3 max-w-2xl">
      <div className="text-sm font-medium text-(--text-primary)">
        Rotating KEK to v{snap.targetVersion}: {phaseLabel}
      </div>
      <Bar
        label="Nodes ready"
        value={snap.nodesAcked}
        total={snap.nodesTotal}
        active={snap.phase === 'distributing'}
      />
      <Bar
        label="Secrets re-encrypted"
        value={snap.secretsRewrapped}
        total={snap.secretsTotal}
        active={snap.phase === 'rewrapping'}
      />
      <div className="text-xs text-(--text-muted)">
        Rotation finishes once every node has the new key. You can leave this page; it keeps
        running.
      </div>
    </div>
  )
}

function Bar({
  label,
  value,
  total,
  active,
}: {
  label: string
  value: number
  total: number
  active: boolean
}) {
  const pct = total > 0 ? Math.round((value / total) * 100) : 100
  return (
    <div>
      <div className="flex justify-between text-xs text-(--text-muted) mb-1">
        <span className={active ? 'text-(--text-primary)' : undefined}>{label}</span>
        <span>
          {value}/{total}
        </span>
      </div>
      <div className="h-2 rounded bg-(--bg-header) overflow-hidden">
        <div
          className="h-full bg-(--bg-btn-primary) transition-all"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  )
}

function StatusBadge({ status }: { status: string }) {
  const tone =
    status === 'approved' ? 'success' : status === 'evicted' ? 'error' : 'warning'
  return <Badge tone={tone}>{status}</Badge>
}
