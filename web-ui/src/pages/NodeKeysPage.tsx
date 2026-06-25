import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNodeKeys } from '../hooks/useNodeKeys'
import { approveNode, revokeNode, rotateKek } from '../api/nodes'
import { formatTime } from '../utils/time'

export function NodeKeysPage() {
  const { data: keys, isLoading, error } = useNodeKeys()
  const qc = useQueryClient()

  const approveMutation = useMutation({
    mutationFn: (id: string) => approveNode(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['node-keys'] }),
  })
  const revokeMutation = useMutation({
    mutationFn: (id: string) => revokeNode(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['node-keys'] }),
  })
  const rotateMutation = useMutation({
    mutationFn: () => rotateKek(),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['node-keys'] }),
  })

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <h2 className="text-2xl font-semibold text-(--text-primary)">Keyholders</h2>
        <button
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
          disabled={rotateMutation.isPending}
          className="px-4 py-2 rounded bg-(--bg-btn-primary) text-(--text-inverse) hover:bg-(--bg-btn-primary-hover) disabled:opacity-50"
        >
          {rotateMutation.isPending ? 'Rotating…' : 'Rotate KEK'}
        </button>
      </div>

      <p className="text-sm text-(--text-muted) max-w-2xl">
        Nodes that may hold the encryption key (KEK) and therefore read/write secrets.
        A joining node registers as <strong>pending</strong>; approve it only after
        verifying its key fingerprint. Approving lets a key-holding node seal the KEK to it.
        Revoking a node stops future sealing; <strong>rotate the KEK</strong> afterward to
        re-encrypt secrets and fully lock the revoked node out.
      </p>

      {rotateMutation.isSuccess &&
        (rotateMutation.data.phase === 'done' ? (
          <div className="text-(--text-success) text-sm">
            KEK rotated to v{rotateMutation.data.targetVersion}. All{' '}
            {rotateMutation.data.secretsTotal} secret(s) re-encrypted.
          </div>
        ) : (
          <div className="text-(--text-warning) text-sm">
            Rotation to v{rotateMutation.data.targetVersion} in progress (
            {rotateMutation.data.phase}): {rotateMutation.data.nodesAcked}/
            {rotateMutation.data.nodesTotal} nodes ready,{' '}
            {rotateMutation.data.secretsRewrapped}/{rotateMutation.data.secretsTotal} secrets
            re-encrypted. It finishes once every node has the new key.
          </div>
        ))}
      {rotateMutation.isError && (
        <div className="text-(--text-danger) text-sm">{String(rotateMutation.error)}</div>
      )}

      {isLoading && <div className="text-(--text-muted)">Loading…</div>}
      {error && <div className="text-(--text-danger)">{String(error)}</div>}

      {keys &&
        (keys.length === 0 ? (
          <div className="text-(--text-muted)">No node keys registered.</div>
        ) : (
          <div className="rounded-lg shadow border border-(--border-color) overflow-hidden bg-(--bg-surface-alt)">
            <table className="w-full text-left">
              <thead className="bg-(--bg-header) text-(--text-primary) border-b border-(--border-subtle)">
                <tr>
                  <th className="px-4 py-2 font-semibold">Node</th>
                  <th className="px-4 py-2 font-semibold">Key</th>
                  <th className="px-4 py-2 font-semibold">Fingerprint</th>
                  <th className="px-4 py-2 font-semibold">Status</th>
                  <th className="px-4 py-2 font-semibold">Approved</th>
                  <th className="px-4 py-2 font-semibold text-right">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-(--border-subtle)">
                {keys.map((k) => (
                  <tr key={`${k.nodeId}-${k.keyVersion}`} className="hover:bg-(--bg-row-hover)">
                    <td className="px-4 py-2 font-mono text-xs">{k.nodeId}</td>
                    <td className="px-4 py-2">v{k.keyVersion}</td>
                    <td className="px-4 py-2 font-mono text-xs text-(--text-muted)">
                      {k.publicKey.slice(0, 16)}…
                    </td>
                    <td className="px-4 py-2">
                      <StatusBadge status={k.status} />
                    </td>
                    <td className="px-4 py-2">{formatTime(k.approvedAt)}</td>
                    <td className="px-4 py-2 text-right">
                      {k.status === 'approved' ? (
                        <button
                          onClick={() => {
                            if (confirm('Revoke this keyholder?')) {
                              revokeMutation.mutate(k.nodeId)
                            }
                          }}
                          className="text-(--text-danger) hover:underline"
                        >
                          Revoke
                        </button>
                      ) : (
                        <button
                          onClick={() => approveMutation.mutate(k.nodeId)}
                          className="text-(--text-accent) hover:underline"
                        >
                          Approve
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ))}
    </div>
  )
}

function StatusBadge({ status }: { status: string }) {
  const cls =
    status === 'approved'
      ? 'bg-(--bg-success) text-(--text-success)'
      : 'bg-(--bg-warning) text-(--text-warning)'
  return <span className={`px-2 py-1 rounded text-xs ${cls}`}>{status}</span>
}
