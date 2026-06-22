use crypto_box::aead::generic_array::GenericArray;
use crypto_box::aead::{Aead as _, AeadCore};
use crypto_box::{ChaChaBox, PublicKey, SecretKey};
use rand_core::OsRng;

use crate::error::{CryptoError, Result};
use crate::key::{KEY_LEN, NodeKeyPair, NodePublicKey, take_key};

/// ChaChaBox nonce length (XChaCha20-Poly1305, 192-bit).
const BOX_NONCE_LEN: usize = 24;

/// Seal a key to a recipient's public key so only the recipient can open it. Behind a
/// trait so a hybrid X25519+ML-KEM wrap can be added when a vetted ML-KEM crate is
/// available (SECRETS.md section 4).
pub trait KeyWrap {
    fn seal(&self, recipient: &NodePublicKey, plaintext: &[u8]) -> Result<Vec<u8>>;
    fn open(&self, keypair: &NodeKeyPair, sealed: &[u8]) -> Result<Vec<u8>>;
}

/// Anonymous sealed box: an ephemeral X25519 keypair with `ChaChaBox` (X25519 ECDH and
/// XChaCha20-Poly1305). Framing is `ephemeral_public(32) || nonce(24) || ciphertext`.
/// The recipient opens it with only their secret key.
pub struct SealedBox;

impl KeyWrap for SealedBox {
    fn seal(&self, recipient: &NodePublicKey, plaintext: &[u8]) -> Result<Vec<u8>> {
        let ephemeral = SecretKey::generate(&mut OsRng);
        let ephemeral_public = ephemeral.public_key();
        let sealed_box = ChaChaBox::new(&recipient.0, &ephemeral);
        let nonce = ChaChaBox::generate_nonce(&mut OsRng);
        let ciphertext = sealed_box
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::Wrap)?;

        let mut out = Vec::with_capacity(KEY_LEN + BOX_NONCE_LEN + ciphertext.len());
        out.extend_from_slice(ephemeral_public.as_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    fn open(&self, keypair: &NodeKeyPair, sealed: &[u8]) -> Result<Vec<u8>> {
        let ephemeral_public_bytes = take_key(sealed, "sealed: ephemeral pubkey")?;
        let rest = &sealed[KEY_LEN..];
        let nonce_bytes = rest
            .get(..BOX_NONCE_LEN)
            .ok_or(CryptoError::Malformed("sealed: nonce"))?;
        let ciphertext = &rest[BOX_NONCE_LEN..];

        let ephemeral_public = PublicKey::from(ephemeral_public_bytes);
        let sealed_box = ChaChaBox::new(&ephemeral_public, keypair.secret());
        let nonce_arr: [u8; BOX_NONCE_LEN] = nonce_bytes
            .try_into()
            .map_err(|_| CryptoError::Malformed("sealed: nonce"))?;
        let nonce = GenericArray::from(nonce_arr);
        sealed_box
            .decrypt(&nonce, ciphertext)
            .map_err(|_| CryptoError::Wrap)
    }
}
