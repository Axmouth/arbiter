# Secrets: threat model, key architecture, and rotation

Status: design (pre-implementation). Tracked in FOLLOWUPS §13. No code yet.

## 1. Goal (what "secure" means here)

The bar: **a database compromise alone must not reveal any secret value.** An attacker
must obtain the DB *plus at least one more thing* (a node's private key, or the KMS). If a
DB dump/backup is enough to read secrets, the feature is pointless.

Corollary that shapes everything: **the master key must never live in the DB** — the DB is
the thing it protects. The DB holds only ciphertext + *sealed* key material + metadata.

### Trust boundary / what may be visible
- **Acceptable to surface:** ciphertext, *sealed* (wrapped) key blobs, public keys, key
  *version/id*, secret *names/metadata*, rotation progress ("62% re-wrapped").
- **Never surfaces:** any plaintext secret value, any plaintext key (DEK/KEK), in: the DB,
  logs/traces, `ps`/argv, env handed to job subprocesses (leaks to grandchildren), API
  responses, `job_runs.config_snapshot`, or backups.
- **Out of scope (trusted):** root / full filesystem access on a node. A node necessarily
  holds *one* local secret (its own private key); compromising a node's disk is "DB + one
  more thing," which is acceptable by definition.

### Invariants
- **I1** KEK plaintext exists only in node memory (after unwrap) + the operator's KEK
  source; never at rest in the DB, never logged, never in argv/env/snapshots/backups.
- **I2** At rest: only ciphertext + sealed keys + version metadata. Names/metadata may be
  visible to operators (RBAC later); values never.
- **I3** Values resolve at the **last moment**, in worker memory, just before use; never
  persisted into the snapshot, never logged; dropped immediately after.
- **I4** The API is **write-only** for secret values (set/rotate); it never returns plaintext.
- **I5** **Fail closed**: a node lacking the needed key version refuses to decrypt (the run
  fails with a clear error); never any plaintext fallback.
- **I6** Rotation never moves a plaintext key through the system; only "rotate to version N"
  commands + progress + *sealed* blobs flow. Old key versions are **retired and deleted**
  once rotation completes (no key hoarding).
- **I7 (enforced)** A job resolves only secrets in its own tenant. `secrets` and `jobs`
  carry `tenant_id`; the worker looks up the run's tenant (`job_tenant`) and resolves via
  `get_secret_by_name(tenant, name)`, so a secret from another tenant is simply not found
  (fail closed). Secret names are unique per tenant. Conformance: `secrets::isolated_per_tenant`.

## 2. Vocabulary

- **DEK — Data Encryption Key:** symmetric key that encrypts one secret's bytes (256-bit
  AEAD: AES-256-GCM or XChaCha20-Poly1305, random nonce per encryption).
- **KEK — Key Encryption Key:** the master key; its only job is to wrap DEKs. Versioned.
- **Envelope encryption:** value -[DEK]-> ciphertext; DEK -[KEK]-> wrapped DEK. Rotating the
  KEK re-wraps the small DEKs; the value ciphertext is never touched or decrypted.
- **Node keypair:** each node's asymmetric (public, private) pair. Public key wraps *to* the
  node; only the node's private key unwraps. (Same idea as age/SOPS/SSH.)

## 3. Key architecture (three layers)

```
 value   --encrypted by-->  DEK              one per secret; nonce-randomized AEAD.
                                             Value ciphertext is NEVER re-touched on KEK rotation.
 DEK     --wrapped by   -->  KEK (version)    master key, in node MEMORY only. Rotation re-wraps DEKs.
 KEK     --wrapped by   -->  each node's      one sealed blob per node, stored in the DB.
                             PUBLIC key       A node opens only its own blob, with its PRIVATE key.
```

### Where each thing lives
| Location | Holds |
|---|---|
| Each node's disk (`0600`) | **that node's private key only** (self-generated, never transmitted) |
| Shared DB | value ciphertext, wrapped DEKs (+ their KEK version), KEK sealed per-node-pubkey, node public keys, key-version + ack metadata |
| Node memory (runtime) | unwrapped KEK version(s); DEK/value only transiently during a single op |
| Nowhere at rest | **KEK plaintext**, **DEK plaintext**, **value plaintext** |

### Why this reconciles "all nodes need the key" with "no shared file, DB-hack-safe"
The leader distributes a key by **writing N sealed blobs to the DB** (one per node, sealed
to that node's public key). The DB — already shared — is the transport, and it is safe
because the blobs are sealed. No shared network file, no node-to-node side channel. A DB
thief sees only blobs they cannot open. The leader needs only **public** keys to seal; it
never holds any node's private key.

### KEK source is pluggable
Default: KEK generated in-cluster and distributed via per-node-pubkey sealing (above);
single-node auto-bootstraps. Alternatives behind the same seam: a KEK from `env`/file
(StackStorm/Rails-style, simplest, but operator must distribute the same key), or an
external **KMS/HSM** (KEK never in arbiter at all). The symmetric layers stay identical.

### Node identity (the one local secret) -- a versioned keyring behind a trait
A node's private key is the only plaintext secret at rest, and it must itself be
**rotation-friendly**, so the identity store is a **versioned keyring**, not a single key.
File format (default `<data_dir>/node_identity.json`, created atomically `0600`):
```json
{
  "current": 2,
  "keys": [
    { "version": 1, "algo": "x25519", "private_key": "<base64>", "created_at": "..." },
    { "version": 2, "algo": "x25519", "private_key": "<base64>", "created_at": "..." }
  ]
}
```
Why versioned: rotating a node's keypair = generate v+1, register its public key, have the
leader re-seal active KEK versions to it, then retire the old once re-sealed. During that
window the node must open blobs sealed to *either* key, so it keeps both private keys. (v1
ships a single version; the format + `node_keys.key_version` are ready for it.)

The store is a trait so the location/medium is swappable without touching callers:
```
trait NodeIdentityStore {
    fn load(&self) -> Result<NodeKeyring>;      // versioned private keys
    fn save(&self, k: &NodeKeyring) -> Result<()>;
}
```
Impls: `FileNodeIdentityStore` (`0600` JSON, default) -> `EnvNodeIdentityStore` (base64 from
env, read-only -- ideal for containers/k8s Secrets) -> OS keyring / TPM / HSM later.

### Honest limitation: co-located SQLite
The "DB compromise alone is useless" guarantee is strongest when the DB is a **separate
trust domain** from the node key: Postgres on another host, or a shipped backup/dump --
those reveal nothing without a node's private key. With **single-node SQLite**, the DB file
and the node-identity file usually sit on the **same host**, so full-host compromise yields
both ("if you have everything the node has, welp"). What is *still* protected even then: a
**stolen DB file / backup / table dump on its own** (the most common leak vector) -- it is
useless without the separately-located node key. To widen the gap on single-node, put the
node-identity on a different volume, or use the `env`/keyring source. We document this
rather than overclaim.

## 4. Quantum note

- Symmetric layers (value/DEK/KEK) at **256-bit** are post-quantum-OK (Grover only halves
  strength → ~128-bit). Keep everything symmetric at 256-bit.
- The **asymmetric per-node wrap** (X25519) is the quantum-exposed layer (Shor), with a
  real "harvest-now-decrypt-later" risk on recorded DB blobs. Mitigations: make the wrap a
  pluggable `KeyWrap` abstraction (default X25519 sealed box; path to **hybrid
  X25519+ML-KEM/Kyber**), retire old versions promptly (shrinks the harvest window), and
  allow a KMS source. Quantum lands only on this one swappable layer; the design holds.

## 5. Schema (sketch)

```
node_keys(
  node_id PK, public_key, algo, status('pending'|'approved'|'evicted'),
  created_at, approved_at
)

kek_versions(
  version PK, state('pending'|'active'|'retired'), algo, created_at, retired_at
)

-- the KEK of `version`, sealed to one node's public key; the node acks once loaded.
kek_shares(
  version, node_id, wrapped_kek, sealed_algo, acked_at,
  PRIMARY KEY (version, node_id)
)

secrets(
  id PK, name UNIQUE, value_ct, value_nonce, aead_algo,
  dek_wrapped, kek_version,            -- DEK sealed by KEK `kek_version`
  created_at, updated_at
)
```

Note: SQL **transactions** make the rotation state machine atomic, crash-safe, and
resumable (each step / batch is a transaction; a half-done rotation just resumes).

## 6. Processes (pseudocode)

### Bootstrap (first node, no KEK yet)
```
on_start_without_kek():
    ensure_local_keypair()                       # generate + write private key 0600 if absent
    kek = random_256()                           # in memory only
    tx:
        insert kek_versions(version=1, state='active')
        insert kek_shares(1, me, wrapped_kek = seal(kek, my_public_key))
    memory_keyring[1] = kek
    set kek_shares(1, me).acked_at = now
```

### Node join (new node, or returning node)
```
on_start():
    ensure_local_keypair()
    upsert node_keys(me, my_public_key, status = 'pending')   # admin approves in UI
    loop forever:
        for v in active_or_pending_versions():
            if v not in memory_keyring and (share := kek_shares[v][me]) exists:
                kek = open(share.wrapped_kek, my_private_key)  # ONLY my private key works
                memory_keyring[v] = kek
                set kek_shares[v][me].acked_at = now           # ack
        sleep(poll)

on_admin_approve(node):                                       # leader, one UI click
    set node_keys(node).status = 'approved'
    tx: for v in active_versions(): insert kek_shares(v, node, seal(memory_keyring[v], node.public_key))
```
The only manual step ever is approving a node's public key — never a key handoff.

### Set / update a secret
```
set_secret(name, value):                                     # value never logged
    dek = random_256()
    nonce = random_nonce()
    ct = aead_encrypt(value, dek, nonce)
    (kek, ver) = active_kek()
    wrapped = wrap(dek, kek)
    upsert secrets(name, value_ct=ct, value_nonce=nonce, dek_wrapped=wrapped, kek_version=ver)
    zeroize(value); zeroize(dek)
```

### Resolve at execution (worker, last moment)
```
resolve(secret_ref):                                         # in worker memory only
    s = load_secret(secret_ref)
    kek = memory_keyring[s.kek_version] or FAIL_CLOSED        # I5
    dek = unwrap(s.dek_wrapped, kek)
    value = aead_decrypt(s.value_ct, s.value_nonce, dek)
    use(value); zeroize(value); zeroize(dek)                 # never persisted/logged (I3)
```

### KEK rotation (leader; transaction-backed; the button + progress bar)
```
rotate_kek():
    new = random_256(); ver = next_version()
    tx:                                                       # publish, sealed per node
        insert kek_versions(ver, state='pending')
        for n in approved_nodes(): insert kek_shares(ver, n, seal(new, n.public_key))
    memory_keyring[ver] = new

    # Barrier part 1: wait until every approved+reachable node has acked `ver`.
    # A permanently-dead node blocks retirement; admin must 'evict' it to proceed.
    wait_until(all approved_nodes have kek_shares[ver].acked_at)

    # Re-wrap DEKs old -> new, in resumable batches (value ciphertext untouched).
    loop:
        tx:
            batch = select * from secrets where kek_version != ver
                    limit N for update skip locked
            if batch empty: break
            for s in batch:
                dek = unwrap(s.dek_wrapped, memory_keyring[s.kek_version])
                update s set dek_wrapped = wrap(dek, new), kek_version = ver
                zeroize(dek)
            emit_progress(done / total)                       # -> UI progress bar

    # Barrier part 2: all secrets on `ver` AND all nodes acked -> retire old.
    tx:
        set kek_versions(ver).state = 'active'
        for old in active_versions() where old != ver:
            set kek_versions(old).state = 'retired', retired_at = now
            delete from kek_shares where version = old        # no key hoarding (I6)
    drop memory_keyring[old] on every node (next poll notices retired -> evicts from memory)
```

### Node offline during rotation
- Offline node simply hasn't acked `ver`; the new version was already sealed to it in step 1,
  so when it returns it reads its blob, unwraps, acks, and is current.
- Old-version **retirement waits** on all approved nodes acking (safe default). A node that
  is gone for good is **evicted** by an admin (UI), which unblocks retirement.
- A returning node that missed *multiple* rotations finds only the still-active version(s)
  sealed to it (retired versions' shares were deleted) — it loads those and is current.

## 7. Ops / UX

- **Single-node:** auto-bootstrap on first run; nothing to configure. "Just works."
- **Add a node:** it self-registers a public key; admin clicks **Approve**; system seals
  active keys to it. No key handoff.
- **Rotate:** admin clicks **Rotate** (or schedule); watch the **progress bar**; done.
- **Evict a dead node:** one click; unblocks a stalled rotation.
- KEK source selectable (in-cluster / env-file / KMS) without changing the rest.

## 8. Integration with the rest of arbiter

- A secret is referenced by id; env values and DB-runner passwords carry a **reference**
  (e.g. `secret:<id>`), resolved by the worker at execution (I3) — never baked into the
  snapshot. Unblocks DB runners (pgsql/mysql) and secret env vars.
- Enforcing conformance angle: assert resolved snapshots never embed plaintext, and that a
  store/round-trip never exposes a value.

## 9. Decisions (locked)
- **AEAD:** XChaCha20-Poly1305 (256-bit key, random 192-bit nonce -- no nonce-reuse footgun)
  for value encryption *and* DEK wrapping. AES-256-GCM behind the `Aead` trait for FIPS.
- **Key wrap (KEK -> node):** X25519 sealed boxes (`crypto_box`), behind a `KeyWrap` trait
  with a path to hybrid X25519+ML-KEM (Kyber).
- **Crates (RustCrypto, pure Rust):** `chacha20poly1305`, `crypto_box`, `getrandom`/`rand`,
  `zeroize` (wipe key/value buffers), optional `secrecy` (non-logging secret types).
  One-crate alternative considered: `dryoc` (libsodium-compatible).
- **DEK model:** per-secret DEK (smaller blast radius; standard); KEK versioned; rotation
  re-wraps DEKs, never the value ciphertext.
- **Node identity:** versioned keyring, `FileNodeIdentityStore` (`0600` JSON) default,
  pluggable (env/keyring/TPM).
- **RBAC (v1):** admin/operator may create/rotate secrets and approve/evict nodes;
  resolution is internal-only; the API never returns plaintext. Full RBAC later.

Still open: whether KEK rotation and per-secret *value* rotation share one UI/flow; exact
node-key rotation UX; KMS provider shape.

## 10. Implementation plan (build order)

1. **Crypto core** (`arbiter-crypto` module/crate): `Aead` (XChaCha20Poly1305) + `KeyWrap`
   (X25519 sealed box) traits + impls; key/`Dek`/`Kek` newtypes with `zeroize`; round-trip,
   seal/open, and tamper-detection (AEAD auth failure) unit tests. No storage yet.
2. **Node identity:** `NodeIdentityStore` trait + `FileNodeIdentityStore` + `NodeKeyring`.
3. **`SecretStore` trait + schema** on pg + sqlite (`secrets`, `node_keys`, `kek_versions`,
   `kek_shares`); conformance: store round-trip + assert no plaintext is ever returned/stored.
4. **Single-node core:** bootstrap (gen KEK v1, seal to self), `set_secret`, `resolve`
   (fail-closed). End-to-end create -> resolve test.
5. **Multi-node:** node join / admin approve / KEK distribution (seal to each pubkey, ack).
6. **KEK rotation** state machine (publish -> ack barrier -> batched re-wrap -> retire),
   progress, evict; transaction-backed + resumable; conformance/integration tests.
7. **Runner integration (done):** a `SecretResolver` trait (core) wired through the worker
   resolves `secret:<name>` references at execution for subprocess env vars and for the DB
   runners' password; `SecretManager` implements it and is built in `node`. pgsql/mysql
   runners execute (`execute_pgsql_query`/`execute_mysql_query`). Single-node ready.
8. **API + UI:** write-only secret endpoints (done: `POST`/`GET /api/v1/secrets`,
   `DELETE /api/v1/secrets/{id}`, tenant-scoped, value never returned, enforcing I4 by
   type); shared-config CRUD storing a `secret:<name>` reference (next); UI panels;
   approve/rotate/evict + progress. (Plus steps 5-6 for clustered deploys.) Create runs
   only on a node holding a KEK, available because the api role runs inside a node
   (`AppState.secrets: Option<SecretAdmin>`); a keyless node returns 503 on create.

Each step is independently testable and commit-able; steps 1-4 deliver usable single-node
secrets before any clustering work.
