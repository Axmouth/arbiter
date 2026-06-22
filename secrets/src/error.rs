use std::fmt;

use arbiter_crypto::CryptoError;

#[derive(Debug)]
pub enum SecretsError {
    Io(String),
    Malformed(String),
    Crypto(CryptoError),
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretsError::Io(msg) => write!(f, "io error: {msg}"),
            SecretsError::Malformed(msg) => write!(f, "malformed: {msg}"),
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

impl From<std::io::Error> for SecretsError {
    fn from(e: std::io::Error) -> Self {
        SecretsError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SecretsError>;
