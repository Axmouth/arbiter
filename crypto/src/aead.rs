use chacha20poly1305::aead::{Aead as _, AeadCore, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::OsRng;

use crate::error::{CryptoError, Result};
use crate::key::SymKey;

/// XChaCha20-Poly1305 nonce length (192-bit).
const NONCE_LEN: usize = 24;

/// An AEAD ciphertext: the random nonce plus the ciphertext and tag.
#[derive(Clone)]
pub struct Ciphertext {
    pub nonce: Vec<u8>,
    pub bytes: Vec<u8>,
}

/// Symmetric authenticated encryption with associated data, behind a trait so
/// AES-256-GCM can be added if FIPS compliance is required.
pub trait Aead {
    fn encrypt(&self, key: &SymKey, plaintext: &[u8], aad: &[u8]) -> Result<Ciphertext>;
    fn decrypt(&self, key: &SymKey, ct: &Ciphertext, aad: &[u8]) -> Result<Vec<u8>>;
}

/// Default AEAD: XChaCha20-Poly1305 with a fresh random 192-bit nonce per encryption.
pub struct XChaChaAead;

impl Aead for XChaChaAead {
    fn encrypt(&self, key: &SymKey, plaintext: &[u8], aad: &[u8]) -> Result<Ciphertext> {
        let cipher =
            XChaCha20Poly1305::new_from_slice(key.expose_bytes()).map_err(|_| CryptoError::Aead)?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let bytes = cipher
            .encrypt(&nonce, Payload { msg: plaintext, aad })
            .map_err(|_| CryptoError::Aead)?;
        Ok(Ciphertext {
            nonce: nonce.to_vec(),
            bytes,
        })
    }

    fn decrypt(&self, key: &SymKey, ct: &Ciphertext, aad: &[u8]) -> Result<Vec<u8>> {
        let cipher =
            XChaCha20Poly1305::new_from_slice(key.expose_bytes()).map_err(|_| CryptoError::Aead)?;
        if ct.nonce.len() != NONCE_LEN {
            return Err(CryptoError::Malformed("nonce length"));
        }
        let nonce = XNonce::from_slice(&ct.nonce);
        cipher
            .decrypt(nonce, Payload { msg: &ct.bytes, aad })
            .map_err(|_| CryptoError::Aead)
    }
}
