use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use arbiter_core::SecretStore;
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
    kek: RwLock<KekState>,
    aead: XChaChaAead,
}

impl SecretManager {
    /// Load the KEK keyring for this node, bootstrapping a fresh KEK on first run.
    pub async fn load_or_bootstrap(
        store: Arc<dyn SecretStore + Send + Sync>,
        node_id: Uuid,
        identity: &NodeKeyring,
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
            let mut keks = HashMap::new();
            keks.insert(1, kek);
            return Ok(Self {
                store,
                node_id,
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
            let opened = open_with_identity(&wrap, identity, &share.wrapped_kek)?;
            keks.insert(v.version, sym_from_vec(opened)?);
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
        // Snapshot the current KEK bytes and version under the lock so the seal loop below
        // (which awaits the store) never holds the guard across an await.
        let (version, kek_bytes) = {
            let guard = self.read_kek();
            let kek = guard.keks.get(&guard.current).ok_or_else(|| {
                SecretsError::KeyUnavailable("current KEK missing".into())
            })?;
            let mut bytes = [0u8; KEY_LEN];
            bytes.copy_from_slice(kek.expose_bytes());
            (guard.current, Zeroizing::new(bytes))
        };
        let wrap = SealedBox;

        // The latest registered public key per node (a node may have several key versions).
        let mut latest: HashMap<Uuid, &arbiter_core::StoredNodeKey> = HashMap::new();
        let node_keys = self.store.list_node_keys().await?;
        for nk in &node_keys {
            if nk.status != "approved" {
                continue;
            }
            latest
                .entry(nk.node_id)
                .and_modify(|cur| {
                    if nk.key_version > cur.key_version {
                        *cur = nk;
                    }
                })
                .or_insert(nk);
        }

        let mut sealed_count = 0;
        for (other_node, nk) in latest {
            if self.store.get_kek_share(version, other_node).await?.is_some() {
                continue;
            }
            let bytes: [u8; KEY_LEN] = nk.public_key.as_slice().try_into().map_err(|_| {
                SecretsError::Malformed(format!("node {other_node} public key is not {KEY_LEN} bytes"))
            })?;
            let pubkey = NodePublicKey::from_bytes(bytes);
            let sealed = wrap.seal(&pubkey, kek_bytes.as_slice())?;
            self.store.put_kek_share(version, other_node, &sealed).await?;
            sealed_count += 1;
        }
        Ok(sealed_count)
    }

    /// Rotate the KEK: generate a fresh version, seal it to every approved node (including
    /// self), re-wrap every secret's DEK under it, then retire the old versions. After this
    /// returns, only the new version can decrypt any secret, so a node revoked beforehand
    /// (status no longer `approved`, see `set_node_key_status`) is locked out. Returns the
    /// new KEK version.
    pub async fn rotate_kek(&self) -> Result<u32> {
        // Durable versions are the source of truth for the next number, so a concurrent
        // bootstrap or an earlier rotation cannot collide.
        let versions = self.store.list_kek_versions().await?;
        let new_version = versions.iter().map(|v| v.version).max().unwrap_or(0) + 1;

        // Persist the new version before holding it in memory, so a crash mid-rotation
        // leaves a recoverable record rather than an orphaned in-memory key.
        self.store.insert_kek_version(new_version, "active").await?;

        // Install the new KEK and make it current. From here on set_secret wraps under it.
        {
            let mut guard = self.write_kek();
            guard.keks.insert(new_version, SymKey::generate());
            guard.current = new_version;
        }

        // Seal the new KEK to every approved node (self included, via its node_keys row).
        self.reconcile_shares().await?;

        // Re-wrap every secret still on an older KEK version. Listing with scope None
        // covers all tenants (this is a system-wide rotation).
        let metas = self.store.list_secret_names(None).await?;
        for meta in metas {
            if meta.kek_version == new_version {
                continue;
            }
            let Some(secret) = self.store.get_secret(meta.id, None).await? else {
                continue;
            };
            if secret.kek_version == new_version {
                continue;
            }
            // Unwrap under the old KEK and re-wrap under the new one, all under the lock.
            // The AEAD AAD is the secret name, matching set_secret.
            let dek_wrapped = {
                let guard = self.read_kek();
                let old = guard.keks.get(&secret.kek_version).ok_or_else(|| {
                    SecretsError::KeyUnavailable(format!(
                        "KEK version {} not held by this node",
                        secret.kek_version
                    ))
                })?;
                let new = guard.keks.get(&new_version).ok_or_else(|| {
                    SecretsError::KeyUnavailable("rotated KEK missing".into())
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
                .rewrap_secret(secret.id, &dek_wrapped, new_version)
                .await?;
        }

        // Retire the old versions now that nothing references them.
        for v in &versions {
            if v.version == new_version || v.state == "retired" {
                continue;
            }
            self.store.set_kek_version_state(v.version, "retired").await?;
        }
        // Drop retired keys from memory so a stale reference can never decrypt with them.
        {
            let mut guard = self.write_kek();
            guard.keks.retain(|k, _| *k == new_version);
        }

        Ok(new_version)
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

    async fn rotate_kek(&self) -> arbiter_core::Result<u32> {
        SecretManager::rotate_kek(self)
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
