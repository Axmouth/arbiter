# KEK rotation, end to end

How a key-encryption-key (KEK) rotation actually runs in arbiter, as built. This is the
operational companion to [SECRETS.md](SECRETS.md) (which has the threat model and the full
key architecture). Read that first if you want the "why".

## The pieces

```
                 value  --DEK-->  ciphertext        (per-secret data key, never rotated on a KEK rotation)
                 DEK    --KEK-->  wrapped DEK        (one versioned KEK wraps every DEK)
                 KEK    --seal--> kek_shares[node]   (KEK sealed to each node's X25519 public key, stored in the DB)
```

Rotating the KEK means: mint a new KEK version, re-wrap every secret's DEK under it, and
retire the old version. The value ciphertext is never touched. The DB only ever holds
ciphertext and sealed blobs, so a stolen DB alone reveals nothing.

## Lifecycle of a KEK version

```
   insert (pending) ──────► active ──────► retiring ──────► retired
        │                     ▲               │                │
   sealed to all         barrier passed   old version      shares deleted
   approved nodes        (every approved   while its        (no key hoarding),
   awaiting their ack    node acked)       secrets are      dropped from memory
                                           re-wrapped
```

Invariant: exactly one version is `active` at any instant. A rotation introduces the next
version as `pending` and only promotes it to `active` once the barrier passes, demoting the
old one to `retiring` in the same step.

Node approval status (separate axis, governs who may hold the KEK):

```
   pending ──approve──► approved ──┬── revoke ──► pending     (security: should not have access)
                                   └── evict  ──► evicted     (liveness: node is gone for good)
```

Only `approved` nodes are sealed new versions and counted in the ack barrier.

## The flow (multi-node)

```
Admin        api node (holds KEK)            DB / kek_shares           follower nodes (B, C)
  │                  │                              │                          │
  │  POST /secrets/rotate                           │                          │
  ├─────────────────►│                              │                          │
  │                  │ insert kek_versions(N,pending)                          │
  │                  ├─────────────────────────────►│                          │
  │                  │ seal N to A,B,C (kek_shares)  │                          │
  │                  ├─────────────────────────────►│                          │
  │                  │ ack N for self (A)            │                          │
  │                  ├─────────────────────────────►│                          │
  │                  │ drive_rotation: barrier 1/3 ──► Distributing             │
  │ ◄────────────────┤  (returns RotationStatus)     │                          │
  │  (UI opens SSE stream: nodes 1/3)                │                          │
  │                  │                              │   30s KEK task            │
  │                  │                              │◄── refresh_keyring ───────┤  B loads share N,
  │                  │                              │    ack N (B), ack N (C)    │  acks; same for C
  │                  │                              │                          │
  │                  │ drive_rotation (any node's KEK task): barrier 3/3 ►pass  │
  │                  │ promote N→active, old→retiring, re-wrap every secret→N   │
  │                  ├─────────────────────────────►│                          │
  │                  │ all secrets on N → retire old + delete its shares        │
  │                  ├─────────────────────────────►│                          │
  │  (SSE stream: nodes 3/3, secrets M/M, then idle → "done")                  │
```

On a **single healthy node** the whole thing collapses into the one synchronous
`POST /secrets/rotate`: the founder is the only approved node and acks itself, so the barrier
passes immediately, secrets are re-wrapped, and the call returns `phase: done`.

## Who drives it

Rotation is not a single long request. `rotate_kek` *initiates* (publish + seal + self-ack)
and then *drives* as far as it can. Completion is carried by `drive_rotation`, which every
node calls on its periodic KEK task. `drive_rotation` is idempotent and safe to run on
several nodes at once (concurrent drives converge on the same end state), so no leader
election is needed and a rotation finishes on its own once the cluster has acked.

A node that is offline during a rotation simply hasn't acked yet; the new version was already
sealed to it, so when it returns, its `refresh_keyring` loads the share, acks, and the
barrier advances. A node that is gone for good is **evicted** by an admin, which removes it
from the approved set so it stops blocking the barrier.

## How the progress bar updates

Progress is never a stored counter (which could drift after a crash). `core::rotation_status`
derives it live from the DB every time it is asked:

- **nodes ready** = approved nodes whose share of the target version has `acked_at` set.
- **secrets re-encrypted** = secrets whose `kek_version` already equals the target, over the
  total.

The Keyholders page opens a `GET /api/v1/secrets/rotation/stream` Server-Sent Events
connection (authenticated by the session cookie the browser sends automatically). The server
emits a fresh snapshot on a short tick and closes the stream once no rotation is in flight.
The two bars fill as nodes ack and secrets are re-wrapped.

## Why a node ends up locked out (revoke then rotate)

1. Admin revokes node X (its status leaves `approved`).
2. Admin rotates. The new KEK is sealed only to still-approved nodes, never to X.
3. Every secret is re-wrapped under the new KEK; the old version's shares are deleted.
4. X holds only the old (now deleted/retired) KEK in memory and can decrypt nothing current.

This is the only way to fully revoke access from a node that already held a key, since you
cannot un-see a key it already loaded — you change the lock instead.

## Where it lives in the code

- `secrets/src/manager.rs` — `rotate_kek`, `drive_rotation`, `refresh_keyring`,
  `seal_version_to_approved`, `rewrap_all_onto`, the `RwLock<KekState>` keyring.
- `core/src/lib.rs` — `RotationStatus`/`RotationPhase`, `rotation_status` (read-only),
  the `SecretStore` primitives (`ack_kek_share`, `delete_kek_shares`, `rewrap_secret`,
  `set_kek_version_state`).
- `api/src/secrets.rs` — `rotate_kek`, `rotation_status`, `rotation_stream` (SSE).
- `api/src/nodes.rs` — approve / revoke / evict.
- `node/src/main.rs` — the per-node KEK upkeep task (reconcile + refresh + drive).
- `web-ui/src/pages/NodeKeysPage.tsx` — the Keyholders page and live progress bars.
