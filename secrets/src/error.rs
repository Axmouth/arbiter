use std::fmt;

use arbiter_crypto::CryptoError;

#[derive(Debug)]
pub enum SecretsError {
    Io(String),
    Store(String),
    Malformed(String),
    NotFound(String),
    KeyUnavailable(String),
    Crypto(CryptoError),
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretsError::Io(msg) => write!(f, "io error: {msg}"),
            SecretsError::Store(msg) => write!(f, "store error: {msg}"),
            SecretsError::Malformed(msg) => write!(f, "malformed: {msg}"),
            SecretsError::NotFound(msg) => write!(f, "not found: {msg}"),
            SecretsError::KeyUnavailable(msg) => write!(f, "key unavailable: {msg}"),
            SecretsError::Crypto(e) => write!(f, "crypto error: {e}"),
        }
    }
}

impl std::error::Error for SecretsError {}

impl From<CryptoError> for SecretsError {
    fn from(e: CryptoError) -> Self {
        SecretsError::Crypto(e)
    }
}

impl From<arbiter_core::ArbiterError> for SecretsError {
    fn from(e: arbiter_core::ArbiterError) -> Self {
        SecretsError::Store(e.to_string())
    }
}

impl From<std::io::Error> for SecretsError {
    fn from(e: std::io::Error) -> Self {
        SecretsError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SecretsError>;
