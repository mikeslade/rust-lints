mod sqlx {
    pub struct PgPool;

    pub fn query(_sql: &str) {}
}

use sqlx::*;

pub fn query_from_core() {
    let _pool_type: Option<PgPool> = None;
    sqlx::query("SELECT id FROM workers");
}

mod nested {
    pub mod deep {
        pub struct Nested;
    }

    pub struct Named;
}

// A glob inside a `use` list is still a wildcard import; HIR keeps the list as
// one item with a nested tree rather than an item per element.
use nested::{Named, deep::*};

pub fn nested_glob_from_core() -> (Named, Nested) {
    (Named, Nested)
}
