use arbiter_core::{SecretAdmin, Store};
use std::sync::Arc;

use crate::auth::jwt::JwtKeys;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store + Send + Sync>,
    pub jwt_keys: JwtKeys,
    /// Encrypt-capable secret writer. `Some` only on a node that holds a KEK; `None`
    /// means this node can list/delete secret metadata but cannot create them.
    pub secrets: Option<Arc<dyn SecretAdmin>>,
}
