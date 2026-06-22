//! Crypto primitives for the arbiter secrets subsystem (see `SECRETS.md`).
//!
//! Two primitives, each behind a trait so the algorithm is swappable:
//! - [`Aead`] ([`XChaChaAead`]): XChaCha20-Poly1305 AEAD for secret values and for wrapping DEKs (random 192-bit nonce).
//! - [`KeyWrap`] ([`SealedBox`]): anonymous sealing of a key to a node's public key (ephemeral X25519 + ChaChaBox); only the recipient's secret key opens it.
//!
//! Key material ([`SymKey`], [`NodeKeyPair`]) is zeroized on drop and never printed.

// Remove this allow once chacha20poly1305 and crypto_box adopt aead 0.6 (generic-array 1.x),
// which drops the deprecated generic-array 0.14 re-export used by the nonce types below.
#![allow(deprecated)]

mod aead;
mod error;
mod key;
mod wrap;

pub use aead::{Aead, Ciphertext, XChaChaAead};
pub use error::{CryptoError, Result};
pub use key::{KEY_LEN, NodeKeyPair, NodePublicKey, SymKey};
pub use wrap::{KeyWrap, SealedBox};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aead_round_trip() {
        let aead = XChaChaAead;
        let key = SymKey::generate();
        let ct = aead.encrypt(&key, b"hello secret", b"aad").expect("encrypt");
        let pt = aead.decrypt(&key, &ct, b"aad").expect("decrypt");
        assert_eq!(pt, b"hello secret");
    }

    #[test]
    fn aead_nonce_is_random_per_encryption() {
        let aead = XChaChaAead;
        let key = SymKey::generate();
        let a = aead.encrypt(&key, b"same", b"").expect("encrypt");
        let b = aead.encrypt(&key, b"same", b"").expect("encrypt");
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.bytes, b.bytes);
    }

    #[test]
    fn aead_wrong_key_fails() {
        let aead = XChaChaAead;
        let key = SymKey::generate();
        let other = SymKey::generate();
        let ct = aead.encrypt(&key, b"data", b"").expect("encrypt");
        assert_eq!(aead.decrypt(&other, &ct, b""), Err(CryptoError::Aead));
    }

    #[test]
    fn aead_wrong_aad_fails() {
        let aead = XChaChaAead;
        let key = SymKey::generate();
        let ct = aead.encrypt(&key, b"data", b"context-a").expect("encrypt");
        assert_eq!(aead.decrypt(&key, &ct, b"context-b"), Err(CryptoError::Aead));
    }

    #[test]
    fn aead_tamper_is_detected() {
        let aead = XChaChaAead;
        let key = SymKey::generate();
        let mut ct = aead.encrypt(&key, b"data", b"").expect("encrypt");
        ct.bytes[0] ^= 0x01;
        assert_eq!(aead.decrypt(&key, &ct, b""), Err(CryptoError::Aead));
    }

    #[test]
    fn sealed_box_round_trip() {
        let wrap = SealedBox;
        let node = NodeKeyPair::generate();
        let sealed = wrap.seal(&node.public(), b"the KEK").expect("seal");
        let opened = wrap.open(&node, &sealed).expect("open");
        assert_eq!(opened, b"the KEK");
    }

    #[test]
    fn sealed_box_only_recipient_opens() {
        let wrap = SealedBox;
        let recipient = NodeKeyPair::generate();
        let other = NodeKeyPair::generate();
        let sealed = wrap.seal(&recipient.public(), b"the KEK").expect("seal");
        assert_eq!(wrap.open(&other, &sealed), Err(CryptoError::Wrap));
    }

    #[test]
    fn sealed_box_tamper_is_detected() {
        let wrap = SealedBox;
        let node = NodeKeyPair::generate();
        let mut sealed = wrap.seal(&node.public(), b"the KEK").expect("seal");
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert_eq!(wrap.open(&node, &sealed), Err(CryptoError::Wrap));
    }

    #[test]
    fn sealed_box_rejects_truncated_input() {
        let wrap = SealedBox;
        let node = NodeKeyPair::generate();
        assert!(matches!(
            wrap.open(&node, b"too-short"),
            Err(CryptoError::Malformed(_))
        ));
    }

    #[test]
    fn node_keypair_secret_round_trips() {
        let node = NodeKeyPair::generate();
        let restored = NodeKeyPair::from_secret_bytes(node.secret_bytes());
        assert_eq!(node.public().to_bytes(), restored.public().to_bytes());
        let wrap = SealedBox;
        let sealed = wrap.seal(&node.public(), b"x").expect("seal");
        assert_eq!(wrap.open(&restored, &sealed).expect("open"), b"x");
    }

    #[test]
    fn sym_key_generate_is_distinct() {
        let a = SymKey::generate();
        let b = SymKey::generate();
        assert_ne!(a.expose_bytes(), b.expose_bytes());
    }
}
