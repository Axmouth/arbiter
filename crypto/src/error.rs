use std::fmt;

/// Crypto errors are intentionally coarse: they never carry plaintext, key bytes,
/// or other sensitive detail that could leak into logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// AEAD encrypt/decrypt failed (includes authentication/tamper failures).
    Aead,
    /// Sealing/opening a sealed box failed.
    Wrap,
    /// Input bytes had the wrong length or framing.
    Malformed(&'static str),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::Aead => write!(f, "aead operation failed"),
            CryptoError::Wrap => write!(f, "key wrap operation failed"),
            CryptoError::Malformed(what) => write!(f, "malformed input: {what}"),
        }
    }
}

impl std::error::Error for CryptoError {}

pub type Result<T> = std::result::Result<T, CryptoError>;
