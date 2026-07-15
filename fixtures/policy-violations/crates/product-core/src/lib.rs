mod sqlx {
    pub struct PgPool;

    pub fn query(_sql: &str) {}
}

use sqlx::*;

pub fn query_from_core() {
    let _pool_type: Option<PgPool> = None;
    sqlx::query("SELECT id FROM workers");
}
