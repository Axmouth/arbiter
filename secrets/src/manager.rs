use std::collections::HashMap;
use std::sync::Arc;

use arbiter_core::SecretStore;
use arbiter_crypto::{Aead, Ciphertext, KEY_LEN, KeyWrap, SealedBox, SymKey, XChaChaAead};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{Result, SecretsError};
use crate::identity::NodeKeyring;

const AEAD_ALGO: &str = "xchacha20poly1305";
const NONCE_LEN: usize = 24;

/// Envelope crypto over a [`SecretStore`]: per-secret DEK, versioned KEK held only in
/// memory. Single-node for now (bootstrap seals the KEK to this node's own key).
pub struct SecretManager {
    store: Arc<dyn SecretStore + Send + Sync>,
    node_id: Uuid,
    keks: HashMap<u32, SymKey>,
    current_kek: u32,
    aead: XChaChaAead,
}

impl SecretManager {
    /// Load the KEK keyring for this node, bootstrapping a fresh KEK on first run.
    pub async fn load_or_bootstrap(
        store: Arc<dyn SecretStore + Send + Sync>,
        node_id: Uuid,
        identity: NodeKeyring,
    ) -> Result<Self> {
        let aead = XChaChaAead;
        let wrap = SealedBox;

        store
            .upsert_node_key(
                node_id,
                identity.current_version(),
                &identity.current_public().to_bytes(),
                "approved",
            )
            .await?;

        let versions = store.list_kek_versions().await?;
        if versions.is_empty() {
            let kek = SymKey::generate();
            let sealed = wrap.seal(&identity.current_public(), kek.expose_bytes())?;
            store.insert_kek_version(1, "active").await?;
            store.put_kek_share(1, node_id, &sealed).await?;
            let mut keks = HashMap::new();
            keks.insert(1, kek);
            return Ok(Self {
                store,
                node_id,
                keks,
                current_kek: 1,
                aead,
            });
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
            current_kek = *keks.keys().max().expect("keks is non-empty");
        }
        Ok(Self {
            store,
            node_id,
            keks,
            current_kek,
            aead,
        })
    }

    /// Encrypt and store a secret by (tenant, name) with a fresh DEK wrapped by the KEK.
    pub async fn set_secret(&self, tenant: Uuid, name: &str, value: &[u8]) -> Result<Uuid> {
        let dek = SymKey::generate();
        let value_ct = self.aead.encrypt(&dek, value, name.as_bytes())?;
        let kek = self.current_kek()?;
        let wrapped = self.aead.encrypt(kek, dek.expose_bytes(), name.as_bytes())?;
        let dek_wrapped = pack(&wrapped);
        let id = self
            .store
            .upsert_secret(
                tenant,
                name,
                &value_ct.bytes,
                &value_ct.nonce,
                AEAD_ALGO,
                &dek_wrapped,
                self.current_kek,
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
        let kek = self.keks.get(&secret.kek_version).ok_or_else(|| {
            SecretsError::KeyUnavailable(format!(
                "KEK version {} not held by this node",
                secret.kek_version
            ))
        })?;
        let (nonce, bytes) = unpack(&secret.dek_wrapped)?;
        let dek_bytes = self
            .aead
            .decrypt(kek, &Ciphertext { nonce, bytes }, name.as_bytes())?;
        let dek = sym_from_vec(dek_bytes)?;
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
        self.current_kek
    }

    pub fn node_id(&self) -> Uuid {
        self.node_id
    }

    fn current_kek(&self) -> Result<&SymKey> {
        self.keks
            .get(&self.current_kek)
            .ok_or_else(|| SecretsError::KeyUnavailable("current KEK missing".into()))
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
