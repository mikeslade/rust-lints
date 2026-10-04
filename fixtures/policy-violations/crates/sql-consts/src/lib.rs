//! SQL text exported for `fixture-db-seam`.

pub const FOREIGN_SQL: &str = "--sql
SELECT id
FROM foreign_workers";

pub static FOREIGN_STATIC_SQL: &str = "--sql
SELECT id FROM foreign_statics";
