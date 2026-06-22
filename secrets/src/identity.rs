use std::io::Write as _;
use std::path::{Path, PathBuf};

use arbiter_crypto::{KEY_LEN, NodeKeyPair, NodePublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::{Result, SecretsError};

const ALGO_X25519: &str = "x25519";

/// One versioned entry in a node's keyring.
pub struct NodeKeyEntry {
    pub version: u32,
    pub created_at: DateTime<Utc>,
    keypair: NodeKeyPair,
}

impl NodeKeyEntry {
    pub fn keypair(&self) -> &NodeKeyPair {
        &self.keypair
    }

    pub fn public(&self) -> NodePublicKey {
        self.keypair.public()
    }
}

/// A node's keyring: one or more versioned X25519 keypairs with a current version.
/// Multiple versions exist only while a node-key rotation is in flight.
pub struct NodeKeyring {
    current: u32,
    keys: Vec<NodeKeyEntry>,
}

impl NodeKeyring {
    /// A fresh single-key keyring (version 1).
    pub fn generate() -> Self {
        Self {
            current: 1,
            keys: vec![NodeKeyEntry {
                version: 1,
                created_at: Utc::now(),
                keypair: NodeKeyPair::generate(),
            }],
        }
    }

    pub fn current_version(&self) -> u32 {
        self.current
    }

    pub fn current(&self) -> &NodeKeyPair {
        self.get(self.current)
            .expect("current version is always present")
    }

    pub fn current_public(&self) -> NodePublicKey {
        self.current().public()
    }

    pub fn get(&self, version: u32) -> Option<&NodeKeyPair> {
        self.keys
            .iter()
            .find(|e| e.version == version)
            .map(|e| &e.keypair)
    }

    pub fn entries(&self) -> impl Iterator<Item = &NodeKeyEntry> {
        self.keys.iter()
    }

    /// Add a new keypair version and make it current. Returns the new version.
    /// Old versions are retained so blobs sealed to them can still be opened until
    /// they are re-sealed to the new public key.
    pub fn add_version(&mut self) -> u32 {
        let version = self.keys.iter().map(|e| e.version).max().unwrap_or(0) + 1;
        self.keys.push(NodeKeyEntry {
            version,
            created_at: Utc::now(),
            keypair: NodeKeyPair::generate(),
        });
        self.current = version;
        version
    }
}

#[derive(Serialize, Deserialize)]
struct KeyringFile {
    current: u32,
    keys: Vec<KeyFile>,
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    version: u32,
    algo: String,
    private_key: String,
    created_at: DateTime<Utc>,
}

impl NodeKeyring {
    fn to_file(&self) -> KeyringFile {
        let keys = self
            .keys
            .iter()
            .map(|e| {
                let mut secret = e.keypair.secret_bytes();
                let private_key = B64.encode(secret);
                secret.zeroize();
                KeyFile {
                    version: e.version,
                    algo: ALGO_X25519.to_string(),
                    private_key,
                    created_at: e.created_at,
                }
            })
            .collect();
        KeyringFile {
            current: self.current,
            keys,
        }
    }

    fn from_file(file: KeyringFile) -> Result<Self> {
        let mut keys = Vec::with_capacity(file.keys.len());
        for k in file.keys {
            if k.algo != ALGO_X25519 {
                return Err(SecretsError::Malformed(format!(
                    "unsupported key algo: {}",
                    k.algo
                )));
            }
            let mut decoded = B64
                .decode(k.private_key.as_bytes())
                .map_err(|_| SecretsError::Malformed("invalid base64 private key".into()))?;
            if decoded.len() != KEY_LEN {
                decoded.zeroize();
                return Err(SecretsError::Malformed("wrong private key length".into()));
            }
            let mut bytes = [0u8; KEY_LEN];
            bytes.copy_from_slice(&decoded);
            decoded.zeroize();
            let keypair = NodeKeyPair::from_secret_bytes(bytes);
            bytes.zeroize();
            keys.push(NodeKeyEntry {
                version: k.version,
                created_at: k.created_at,
                keypair,
            });
        }
        if !keys.iter().any(|e| e.version == file.current) {
            return Err(SecretsError::Malformed(
                "current version not present in keyring".into(),
            ));
        }
        Ok(Self {
            current: file.current,
            keys,
        })
    }
}

/// Where a node's private keyring lives. Pluggable so the medium can change
/// (file, env, OS keyring, TPM) without touching callers.
pub trait NodeIdentityStore {
    /// Load the keyring, or `None` if none exists yet.
    fn load(&self) -> Result<Option<NodeKeyring>>;
    fn save(&self, keyring: &NodeKeyring) -> Result<()>;
}

/// Load the existing keyring, or generate and persist a fresh one on first run.
pub fn load_or_generate(store: &dyn NodeIdentityStore) -> Result<NodeKeyring> {
    match store.load()? {
        Some(keyring) => Ok(keyring),
        None => {
            let keyring = NodeKeyring::generate();
            store.save(&keyring)?;
            Ok(keyring)
        }
    }
}

/// Stores the keyring as a `0600` JSON file (the default node identity store).
pub struct FileNodeIdentityStore {
    path: PathBuf,
}

impl FileNodeIdentityStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl NodeIdentityStore for FileNodeIdentityStore {
    fn load(&self) -> Result<Option<NodeKeyring>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&self.path)?;
        let file: KeyringFile = serde_json::from_str(&contents)
            .map_err(|e| SecretsError::Malformed(format!("invalid identity file: {e}")))?;
        Ok(Some(NodeKeyring::from_file(file)?))
    }

    fn save(&self, keyring: &NodeKeyring) -> Result<()> {
        let json = serde_json::to_string_pretty(&keyring.to_file())
            .map_err(|e| SecretsError::Malformed(e.to_string()))?;
        write_private_file(&self.path, json.as_bytes())
    }
}

/// Write `contents` to `path` atomically with `0600` permissions: a tempfile in the
/// same directory (created `0600`) is written then renamed over the target.
fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".identity-")
        .tempfile_in(dir)?;
    tmp.write_all(contents)?;
    tmp.flush()?;
    tmp.persist(path)
        .map_err(|e| SecretsError::Io(e.to_string()))?;
    Ok(())
}
