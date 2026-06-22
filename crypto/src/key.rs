use crypto_box::{PublicKey, SecretKey};
use rand_core::{OsRng, RngCore};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{CryptoError, Result};

/// Length of a symmetric key (256-bit) and of an X25519 public/secret key.
pub const KEY_LEN: usize = 32;

/// A 256-bit symmetric key. Zeroized on drop, no `Debug`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SymKey([u8; KEY_LEN]);

impl SymKey {
    /// Generate a fresh random key from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        let key = Self(bytes);
        bytes.zeroize();
        key
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn expose_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

/// A node's X25519 public key.
#[derive(Clone)]
pub struct NodePublicKey(pub(crate) PublicKey);

impl NodePublicKey {
    pub fn to_bytes(&self) -> [u8; KEY_LEN] {
        *self.0.as_bytes()
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(PublicKey::from(bytes))
    }
}

/// A node's X25519 keypair. The secret half is zeroized on drop and never printed.
pub struct NodeKeyPair {
    secret: SecretKey,
}

impl NodeKeyPair {
    /// Generate a fresh node identity from the OS CSPRNG.
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::generate(&mut OsRng),
        }
    }

    pub fn from_secret_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self {
            secret: SecretKey::from(bytes),
        }
    }

    /// Export the secret bytes for the identity store. The caller zeroizes them.
    pub fn secret_bytes(&self) -> [u8; KEY_LEN] {
        self.secret.to_bytes()
    }

    pub fn public(&self) -> NodePublicKey {
        NodePublicKey(self.secret.public_key())
    }

    pub(crate) fn secret(&self) -> &SecretKey {
        &self.secret
    }
}

/// Read exactly `KEY_LEN` bytes from the front of a slice.
pub(crate) fn take_key(bytes: &[u8], what: &'static str) -> Result<[u8; KEY_LEN]> {
    let slice = bytes.get(..KEY_LEN).ok_or(CryptoError::Malformed(what))?;
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(slice);
    Ok(out)
}
