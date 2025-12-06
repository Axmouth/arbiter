use dromio_core::Store;
use std::sync::Arc;

use crate::auth::jwt::JwtKeys;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store + Send + Sync>,
    pub jwt_keys: JwtKeys,
}
