mod sqlx {
    pub struct PgPool;

    pub fn query(_sql: &str) {}
}

use sqlx::*;

pub fn query_from_core() {
    let _pool_type: Option<PgPool> = None;
    sqlx::query("SELECT id FROM workers");
}

/// A document rendered from an envelope, handed to a writer alongside the row
/// it belongs to.
pub struct RenderedPayload {
    pub digest: [u8; 32],
}

/// Stand-in for a trait a consuming project closes to a reviewed set of
/// implementors: public, unsealed, and implementable from any crate that can
/// name it. The closed-trait pass decides membership on resolved `DefId`s, so
/// the fixtures below can spell an implementation any way Rust allows.
pub trait OutboxRepository {
    fn store(&self, payload: &RenderedPayload);
}
