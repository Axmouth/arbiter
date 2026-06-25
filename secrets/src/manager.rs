use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use arbiter_core::{RotationPhase, RotationStatus, SecretStore};
use arbiter_crypto::{
    Aead, Ciphertext, KEY_LEN, KeyWrap, NodePublicKey, SealedBox, SymKey, XChaChaAead,
};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{Result, SecretsError};
use crate::identity::NodeKeyring;

const AEAD_ALGO: &str = "xchacha20poly1305";
const NONCE_LEN: usize = 24;

/// The in-memory KEK keyring: every KEK version this node holds plus which one is current.
/// Held behind a lock so rotation can swap in a new version while reads stay live. Crypto
/// must run while the lock is held but the guard is dropped before any `.await` (it is not
/// `Send`).
struct KekState {
    keks: HashMap<u32, SymKey>,
    current: u32,
}

/// Envelope crypto over a [`SecretStore`]: per-secret DEK, versioned KEK held only in
/// memory. The KEK keyring lives behind an [`RwLock`] so rotation can install a new version
/// in place on a shared manager.
pub struct SecretManager {
    store: Arc<dyn SecretStore + Send + Sync>,
    node_id: Uuid,
    identity: Arc<NodeKeyring>,
    kek: RwLock<KekState>,
    aead: XChaChaAead,
}

impl SecretManager {
    /// Load the KEK keyring for this node, bootstrapping a fresh KEK on first run.
    pub async fn load_or_bootstrap(
        store: Arc<dyn SecretStore + Send + Sync>,
        node_id: Uuid,
        identity: Arc<NodeKeyring>,
    ) -> Result<Self> {
        let aead = XChaChaAead;
        let wrap = SealedBox;

        let versions = store.list_kek_versions().await?;
        if versions.is_empty() {
            // Founder: bootstrap the KEK and self-approve (someone has to be trusted to
            // approve the rest).
            store
                .upsert_node_key(
                    node_id,
                    identity.current_version(),
                    &identity.current_public().to_bytes(),
                    "approved",
                )
                .await?;
            let kek = SymKey::generate();
            let sealed = wrap.seal(&identity.current_public(), kek.expose_bytes())?;
            store.insert_kek_version(1, "active").await?;
            store.put_kek_share(1, node_id, &sealed).await?;
            store.ack_kek_share(1, node_id).await?;
            let mut keks = HashMap::new();
            keks.insert(1, kek);
            return Ok(Self {
                store,
                node_id,
                identity,
                kek: RwLock::new(KekState { keks, current: 1 }),
                aead,
            });
        }

        // Joining an existing cluster: register our public key as pending (awaiting an
        // admin approval before a holder will seal the KEK to us). Do not re-register if
        // already present, so a restart never downgrades an approved node.
        let already_registered = store
            .list_node_keys()
            .await?
            .into_iter()
            .any(|k| k.node_id == node_id && k.key_version == identity.current_version());
        if !already_registered {
            store
                .upsert_node_key(
                    node_id,
                    identity.current_version(),
                    &identity.current_public().to_bytes(),
                    "pending",
                )
                .await?;
        }

        let mut keks = HashMap::new();
        let mut current_kek = 0;
        for v in &versions {
            if v.state == "retired" {
                continue;
            }
            let Some(share) = store.get_kek_share(v.version, node_id).await? else {
                continue;
            };
            let opened = open_with_identity(&wrap, &identity, &share.wrapped_kek)?;
            keks.insert(v.version, sym_from_vec(opened)?);
            // Ack the share so the rotation barrier knows this node can read this version.
            store.ack_kek_share(v.version, node_id).await?;
            if v.state == "active" {
                current_kek = v.version;
            }
        }
        if keks.is_empty() {
            return Err(SecretsError::KeyUnavailable(
                "no KEK version is sealed to this node".into(),
            ));
        }
        if current_kek == 0 {
            current_kek = keks
                .keys()
                .max()
                .copied()
                .ok_or_else(|| SecretsError::KeyUnavailable("no KEK version held".into()))?;
        }
        Ok(Self {
            store,
            node_id,
            identity,
            kek: RwLock::new(KekState {
                keks,
                current: current_kek,
            }),
            aead,
        })
    }

    /// Encrypt and store a secret by (tenant, name) with a fresh DEK wrapped by the KEK.
    pub async fn set_secret(&self, tenant: Uuid, name: &str, value: &[u8]) -> Result<Uuid> {
        let dek = SymKey::generate();
        let value_ct = self.aead.encrypt(&dek, value, name.as_bytes())?;
        // Wrap the DEK under the current KEK while holding the read lock, then drop it
        // before the store await (the guard is not Send).
        let (dek_wrapped, version) = {
            let guard = self.read_kek();
            let kek = guard.keks.get(&guard.current).ok_or_else(|| {
                SecretsError::KeyUnavailable("current KEK missing".into())
            })?;
            let wrapped = self.aead.encrypt(kek, dek.expose_bytes(), name.as_bytes())?;
            (pack(&wrapped), guard.current)
        };
        let id = self
            .store
            .upsert_secret(
                tenant,
                name,
                &value_ct.bytes,
                &value_ct.nonce,
                AEAD_ALGO,
                &dek_wrapped,
                version,
            )
            .await?;
        Ok(id)
    }

    /// Resolve a secret value at the last moment. Fails closed if this node does not
    /// hold the KEK version the secret was wrapped with. The returned value zeroizes
    /// on drop.
    pub async fn resolve(&self, tenant: Uuid, name: &str) -> Result<Zeroizing<Vec<u8>>> {
        let secret = self
            .store
            .get_secret_by_name(tenant, name)
            .await?
            .ok_or_else(|| SecretsError::NotFound(name.to_string()))?;
        // Unwrap the DEK under the secret's KEK version while holding the read lock, then
        // drop it before any further await (the guard is not Send).
        let dek = {
            let guard = self.read_kek();
            let kek = guard.keks.get(&secret.kek_version).ok_or_else(|| {
                SecretsError::KeyUnavailable(format!(
                    "KEK version {} not held by this node",
                    secret.kek_version
                ))
            })?;
            let (nonce, bytes) = unpack(&secret.dek_wrapped)?;
            let dek_bytes = self
                .aead
                .decrypt(kek, &Ciphertext { nonce, bytes }, name.as_bytes())?;
            sym_from_vec(dek_bytes)?
        };
        let value = self.aead.decrypt(
            &dek,
            &Ciphertext {
                nonce: secret.value_nonce,
                bytes: secret.value_ct,
            },
            name.as_bytes(),
        )?;
        Ok(Zeroizing::new(value))
    }

    pub fn current_kek_version(&self) -> u32 {
        self.read_kek().current
    }

    pub fn node_id(&self) -> Uuid {
        self.node_id
    }

    /// Seal the active KEK to every approved node that does not yet hold a share of it,
    /// so a node that has registered its public key (e.g. a freshly joined worker or an
    /// api-only node) can load the KEK. Run by any node that holds the active KEK; it is
    /// idempotent (nodes that already have a share are skipped). Returns how many shares
    /// were newly written.
    pub async fn reconcile_shares(&self) -> Result<usize> {
        let version = self.read_kek().current;
        self.seal_version_to_approved(version).await
    }

    /// Seal one held KEK version to every approved node that does not yet hold a share of it.
    /// Idempotent (nodes that already have a share are skipped). Returns how many shares were
    /// newly written.
    async fn seal_version_to_approved(&self, version: u32) -> Result<usize> {
        // Snapshot the KEK bytes under the lock so the seal loop below (which awaits the
        // store) never holds the guard across an await.
        let kek_bytes = {
            let guard = self.read_kek();
            let kek = guard.keks.get(&version).ok_or_else(|| {
                SecretsError::KeyUnavailable(format!("KEK version {version} not held"))
            })?;
            let mut bytes = [0u8; KEY_LEN];
            bytes.copy_from_slice(kek.expose_bytes());
            Zeroizing::new(bytes)
        };
        let wrap = SealedBox;

        let mut sealed_count = 0;
        for nk in self.approved_node_keys().await? {
            if self.store.get_kek_share(version, nk.node_id).await?.is_some() {
                continue;
            }
            let bytes: [u8; KEY_LEN] = nk.public_key.as_slice().try_into().map_err(|_| {
                SecretsError::Malformed(format!(
                    "node {} public key is not {KEY_LEN} bytes",
                    nk.node_id
                ))
            })?;
            let pubkey = NodePublicKey::from_bytes(bytes);
            let sealed = wrap.seal(&pubkey, kek_bytes.as_slice())?;
            self.store.put_kek_share(version, nk.node_id, &sealed).await?;
            sealed_count += 1;
        }
        Ok(sealed_count)
    }

    /// The latest approved key per node (a node may register several key versions).
    async fn approved_node_keys(&self) -> Result<Vec<arbiter_core::StoredNodeKey>> {
        let mut latest: HashMap<Uuid, arbiter_core::StoredNodeKey> = HashMap::new();
        for nk in self.store.list_node_keys().await? {
            if nk.status != "approved" {
                continue;
            }
            latest
                .entry(nk.node_id)
                .and_modify(|cur| {
                    if nk.key_version > cur.key_version {
                        *cur = nk.clone();
                    }
                })
                .or_insert(nk);
        }
        Ok(latest.into_values().collect())
    }

    /// Pick up KEK versions that were sealed to this node since it last loaded (e.g. a
    /// rotation published by another node) and drop versions that have been retired. Loads
    /// each newly-sealed share into memory, acks it (so a rotation barrier can proceed), and
    /// retracks the current version. Run periodically by every node so a long-running node
    /// follows rotations without a restart. Returns how many new versions were loaded.
    pub async fn refresh_keyring(&self) -> Result<usize> {
        let wrap = SealedBox;
        let versions = self.store.list_kek_versions().await?;
        let held: HashSet<u32> = self.read_kek().keks.keys().copied().collect();

        // Gather the work with the lock released (the store awaits below are not Send-safe
        // under the guard); apply it in one short write-lock at the end.
        let mut newly: Vec<(u32, SymKey)> = Vec::new();
        let mut retired: HashSet<u32> = HashSet::new();
        let mut active_version: Option<u32> = None;
        for v in &versions {
            if v.state == "retired" {
                retired.insert(v.version);
                continue;
            }
            if v.state == "active" {
                active_version = Some(v.version);
            }
            if held.contains(&v.version) {
                continue;
            }
            let Some(share) = self.store.get_kek_share(v.version, self.node_id).await? else {
                continue;
            };
            let opened = open_with_identity(&wrap, &self.identity, &share.wrapped_kek)?;
            newly.push((v.version, sym_from_vec(opened)?));
            self.store.ack_kek_share(v.version, self.node_id).await?;
        }

        let count = newly.len();
        {
            let mut guard = self.write_kek();
            for (ver, key) in newly {
                guard.keks.insert(ver, key);
            }
            guard.keks.retain(|k, _| !retired.contains(k));
            // Track the active version once we actually hold it.
            if let Some(av) = active_version
                && guard.keks.contains_key(&av)
            {
                guard.current = av;
            }
            // If the current version was retired out from under us, fall back to the newest
            // version we still hold.
            if !guard.keks.contains_key(&guard.current)
                && let Some(max) = guard.keks.keys().max().copied()
            {
                guard.current = max;
            }
        }
        Ok(count)
    }

    /// Start a KEK rotation and drive it as far as it can right now. Publishes a new KEK
    /// version (state `pending`), seals it to every approved node, then advances: once all
    /// approved nodes have acked the new version it is activated, every secret is re-wrapped
    /// under it, and the old versions are retired (their shares deleted). On a single healthy
    /// node this completes synchronously and returns `Done`. On a cluster it may return
    /// `Distributing` while waiting for other nodes to ack (their periodic `refresh_keyring`
    /// loads and acks the new version, then a `drive_rotation` finishes it). A node revoked
    /// before rotation is never sealed the new version and is locked out once the old version
    /// retires.
    pub async fn rotate_kek(&self) -> Result<RotationStatus> {
        // Reject a second rotation while one is in flight (a non-retired, non-active version
        // already exists). The caller can poll drive_rotation to finish the current one.
        let versions = self.store.list_kek_versions().await?;
        if versions.iter().any(|v| v.state == "pending") {
            return self.drive_rotation().await;
        }

        let new_version = versions.iter().map(|v| v.version).max().unwrap_or(0) + 1;
        // Persist the new version (pending) before holding it in memory, so a crash mid
        // rotation leaves a recoverable record rather than an orphaned in-memory key. It is
        // NOT active yet, so set_secret keeps wrapping under the still-active old version
        // until the ack barrier passes.
        self.store.insert_kek_version(new_version, "pending").await?;
        {
            let mut guard = self.write_kek();
            guard.keks.insert(new_version, SymKey::generate());
        }
        // Seal it to every approved node (self included via its node_keys row) and self-ack.
        self.seal_version_to_approved(new_version).await?;
        self.store.ack_kek_share(new_version, self.node_id).await?;

        self.drive_rotation().await
    }

    /// Advance an in-flight rotation as far as it can go now, returning progress. Idempotent
    /// and safe to call from a periodic driver. Returns `Idle` when nothing is in flight.
    pub async fn drive_rotation(&self) -> Result<RotationStatus> {
        let versions = self.store.list_kek_versions().await?;
        let pending = versions.iter().find(|v| v.state == "pending");
        let retiring: Vec<u32> = versions
            .iter()
            .filter(|v| v.state == "retiring")
            .map(|v| v.version)
            .collect();
        let active = versions.iter().find(|v| v.state == "active").map(|v| v.version);

        // Determine the rotation target: a pending version being introduced, or (if we are
        // resuming after a crash between activate and retire) the active version with leftover
        // retiring versions.
        let target = if let Some(p) = pending {
            p.version
        } else if !retiring.is_empty() {
            match active {
                Some(a) => a,
                None => return Ok(self.idle_status()),
            }
        } else {
            return Ok(self.idle_status());
        };

        // Barrier: every approved node must have acked the target version before it becomes
        // the one secrets are re-wrapped under, so no live node is locked out mid-cutover.
        let approved = self.approved_node_keys().await?;
        let nodes_total = approved.len() as u32;
        let mut nodes_acked = 0u32;
        for nk in &approved {
            if let Some(share) = self.store.get_kek_share(target, nk.node_id).await?
                && share.acked_at.is_some()
            {
                nodes_acked += 1;
            }
        }

        let (secrets_rewrapped, secrets_total) = self.rewrap_progress(target).await?;

        if pending.is_some() && nodes_acked < nodes_total {
            // Still distributing: keep sealing to any approved node missing a share so a
            // newly approved node can catch up, then wait for acks.
            self.seal_version_to_approved(target).await?;
            return Ok(RotationStatus {
                phase: RotationPhase::Distributing,
                target_version: Some(target),
                nodes_acked,
                nodes_total,
                secrets_rewrapped,
                secrets_total,
            });
        }

        // Barrier passed. Promote the pending version to active and demote the old active to
        // retiring, so exactly one version is active and set_secret now wraps under the new.
        if let Some(p) = pending {
            self.store.set_kek_version_state(p.version, "active").await?;
            if let Some(a) = active
                && a != p.version
            {
                self.store.set_kek_version_state(a, "retiring").await?;
            }
            self.write_kek().current = target;
        }

        // Re-wrap every secret still on an older version onto the target. Listing with scope
        // None covers all tenants (this is a system-wide rotation).
        self.rewrap_all_onto(target).await?;

        let (rewrapped_after, total_after) = self.rewrap_progress(target).await?;
        if rewrapped_after < total_after {
            return Ok(RotationStatus {
                phase: RotationPhase::Rewrapping,
                target_version: Some(target),
                nodes_acked,
                nodes_total,
                secrets_rewrapped: rewrapped_after,
                secrets_total: total_after,
            });
        }

        // All secrets on the target and all nodes acked: retire the old versions and delete
        // their shares (no key hoarding), then drop them from memory.
        let retire_now: Vec<u32> = versions
            .iter()
            .filter(|v| v.version != target && v.state != "retired")
            .map(|v| v.version)
            .collect();
        for old in &retire_now {
            self.store.set_kek_version_state(*old, "retired").await?;
            self.store.delete_kek_shares(*old).await?;
        }
        {
            let mut guard = self.write_kek();
            guard.keks.retain(|k, _| *k == target);
            guard.current = target;
        }

        Ok(RotationStatus {
            phase: RotationPhase::Done,
            target_version: Some(target),
            nodes_acked,
            nodes_total,
            secrets_rewrapped: total_after,
            secrets_total: total_after,
        })
    }

    fn idle_status(&self) -> RotationStatus {
        RotationStatus {
            phase: RotationPhase::Idle,
            target_version: None,
            nodes_acked: 0,
            nodes_total: 0,
            secrets_rewrapped: 0,
            secrets_total: 0,
        }
    }

    /// How many secrets are already on `target` out of the total (for progress reporting).
    async fn rewrap_progress(&self, target: u32) -> Result<(u32, u32)> {
        let metas = self.store.list_secret_names(None).await?;
        let total = metas.len() as u32;
        let done = metas.iter().filter(|m| m.kek_version == target).count() as u32;
        Ok((done, total))
    }

    /// Re-wrap every secret not already on `target` onto it (value ciphertext untouched).
    async fn rewrap_all_onto(&self, target: u32) -> Result<()> {
        let metas = self.store.list_secret_names(None).await?;
        for meta in metas {
            if meta.kek_version == target {
                continue;
            }
            let Some(secret) = self.store.get_secret(meta.id, None).await? else {
                continue;
            };
            if secret.kek_version == target {
                continue;
            }
            // Unwrap under the old KEK and re-wrap under the target, all under the lock. The
            // AEAD AAD is the secret name, matching set_secret.
            let dek_wrapped = {
                let guard = self.read_kek();
                let old = guard.keks.get(&secret.kek_version).ok_or_else(|| {
                    SecretsError::KeyUnavailable(format!(
                        "KEK version {} not held by this node",
                        secret.kek_version
                    ))
                })?;
                let new = guard.keks.get(&target).ok_or_else(|| {
                    SecretsError::KeyUnavailable("rotation target KEK missing".into())
                })?;
                let (nonce, bytes) = unpack(&secret.dek_wrapped)?;
                let dek_bytes =
                    self.aead
                        .decrypt(old, &Ciphertext { nonce, bytes }, secret.name.as_bytes())?;
                let dek = sym_from_vec(dek_bytes)?;
                let rewrapped =
                    self.aead
                        .encrypt(new, dek.expose_bytes(), secret.name.as_bytes())?;
                pack(&rewrapped)
            };
            self.store
                .rewrap_secret(secret.id, &dek_wrapped, target)
                .await?;
        }
        Ok(())
    }

    /// Read the KEK keyring, recovering the lock if a previous holder panicked (the keyring
    /// is plain data, so a poisoned lock carries no torn invariant).
    fn read_kek(&self) -> RwLockReadGuard<'_, KekState> {
        self.kek.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_kek(&self) -> RwLockWriteGuard<'_, KekState> {
        self.kek.write().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait::async_trait]
impl arbiter_core::SecretAdmin for SecretManager {
    async fn set_secret(&self, tenant: Uuid, name: &str, value: &[u8]) -> arbiter_core::Result<Uuid> {
        SecretManager::set_secret(self, tenant, name, value)
            .await
            .map_err(|e| arbiter_core::ArbiterError::ExecutionError(e.to_string()))
    }

    async fn rotate_kek(&self) -> arbiter_core::Result<RotationStatus> {
        SecretManager::rotate_kek(self)
            .await
            .map_err(|e| arbiter_core::ArbiterError::ExecutionError(e.to_string()))
    }

    async fn drive_rotation(&self) -> arbiter_core::Result<RotationStatus> {
        SecretManager::drive_rotation(self)
            .await
            .map_err(|e| arbiter_core::ArbiterError::ExecutionError(e.to_string()))
    }
}

#[async_trait::async_trait]
impl arbiter_core::SecretResolver for SecretManager {
    async fn resolve_secret(&self, tenant: Uuid, name: &str) -> arbiter_core::Result<String> {
        let value = self
            .resolve(tenant, name)
            .await
            .map_err(|e| arbiter_core::ArbiterError::ExecutionError(e.to_string()))?;
        String::from_utf8(value.to_vec()).map_err(|_| {
            arbiter_core::ArbiterError::ExecutionError("secret value is not valid UTF-8".into())
        })
    }
}

/// Try every node key version until one opens the sealed blob.
fn open_with_identity(wrap: &SealedBox, identity: &NodeKeyring, sealed: &[u8]) -> Result<Vec<u8>> {
    for entry in identity.entries() {
        if let Ok(plaintext) = wrap.open(entry.keypair(), sealed) {
            return Ok(plaintext);
        }
    }
    Err(SecretsError::KeyUnavailable(
        "no node key opens the sealed KEK".into(),
    ))
}

fn pack(ct: &Ciphertext) -> Vec<u8> {
    let mut out = Vec::with_capacity(ct.nonce.len() + ct.bytes.len());
    out.extend_from_slice(&ct.nonce);
    out.extend_from_slice(&ct.bytes);
    out
}

fn unpack(blob: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    if blob.len() < NONCE_LEN {
        return Err(SecretsError::Malformed("wrapped dek too short".into()));
    }
    Ok((blob[..NONCE_LEN].to_vec(), blob[NONCE_LEN..].to_vec()))
}

fn sym_from_vec(mut bytes: Vec<u8>) -> Result<SymKey> {
    if bytes.len() != KEY_LEN {
        bytes.zeroize();
        return Err(SecretsError::Malformed("wrong key length".into()));
    }
    let mut arr = [0u8; KEY_LEN];
    arr.copy_from_slice(&bytes);
    bytes.zeroize();
    let key = SymKey::from_bytes(arr);
    arr.zeroize();
    Ok(key)
}
