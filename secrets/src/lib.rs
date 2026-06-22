//! Secrets management for arbiter (see `SECRETS.md`), built on `arbiter-crypto`.
//!
//! Step 2: node identity. A node holds a versioned [`NodeKeyring`] (its X25519
//! keypairs), persisted through a pluggable [`NodeIdentityStore`] (default
//! [`FileNodeIdentityStore`], a `0600` JSON file). This is the one local secret a node
//! keeps outside the shared DB.

mod error;
mod identity;

pub use error::{Result, SecretsError};
pub use identity::{
    FileNodeIdentityStore, NodeIdentityStore, NodeKeyEntry, NodeKeyring, load_or_generate,
};

#[cfg(test)]
mod tests {
    use super::*;
    use arbiter_crypto::{KeyWrap, SealedBox};

    fn temp_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("node_identity.json");
        (dir, path)
    }

    #[test]
    fn load_is_none_before_first_save() {
        let (_dir, path) = temp_path();
        let store = FileNodeIdentityStore::new(&path);
        assert!(store.load().expect("load").is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_dir, path) = temp_path();
        let store = FileNodeIdentityStore::new(&path);
        let keyring = NodeKeyring::generate();
        store.save(&keyring).expect("save");

        let loaded = store.load().expect("load").expect("present");
        assert_eq!(loaded.current_version(), keyring.current_version());
        assert_eq!(
            loaded.current_public().to_bytes(),
            keyring.current_public().to_bytes()
        );

        // The loaded keyring opens a blob sealed to the original's public key.
        let sealed = SealedBox
            .seal(&keyring.current_public(), b"kek")
            .expect("seal");
        assert_eq!(
            SealedBox.open(loaded.current(), &sealed).expect("open"),
            b"kek"
        );
    }

    #[test]
    fn load_or_generate_persists_on_first_run() {
        let (_dir, path) = temp_path();
        let store = FileNodeIdentityStore::new(&path);
        let first = load_or_generate(&store).expect("generate");
        let second = load_or_generate(&store).expect("load");
        assert_eq!(
            first.current_public().to_bytes(),
            second.current_public().to_bytes(),
            "second call loads the persisted identity, not a new one"
        );
    }

    #[test]
    fn add_version_bumps_current_and_keeps_old() {
        let mut keyring = NodeKeyring::generate();
        let v1_pub = keyring.get(1).expect("v1").public().to_bytes();
        let v2 = keyring.add_version();
        assert_eq!(v2, 2);
        assert_eq!(keyring.current_version(), 2);
        assert!(keyring.get(1).is_some(), "old version retained");
        assert_eq!(keyring.get(1).expect("v1").public().to_bytes(), v1_pub);
        assert_ne!(
            keyring.get(2).expect("v2").public().to_bytes(),
            v1_pub,
            "new version is a distinct keypair"
        );
    }

    #[test]
    fn rejects_malformed_file() {
        let (_dir, path) = temp_path();
        std::fs::write(&path, b"not json").expect("write");
        let store = FileNodeIdentityStore::new(&path);
        assert!(matches!(store.load(), Err(SecretsError::Malformed(_))));
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, path) = temp_path();
        let store = FileNodeIdentityStore::new(&path);
        store.save(&NodeKeyring::generate()).expect("save");
        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "identity file must be 0600");
    }
}
