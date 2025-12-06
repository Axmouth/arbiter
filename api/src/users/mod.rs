use dromio_config::AdminConfig;
use dromio_core::{Result, Store, UserRole};

use crate::auth::hash_password;

pub mod model;
pub mod routes;

pub async fn seed_admin(store: &dyn Store, cfg: &AdminConfig) -> Result<()> {
    let count = store.count_users().await?;

    if count == 0 {
        let user = &cfg.username;
        let pass = &cfg.password;

        let hash = hash_password(pass);

        store.create_user(user, &hash, UserRole::Admin).await?;

        println!("Created default admin user: {user}/{pass}");
    }
    Ok(())
}
