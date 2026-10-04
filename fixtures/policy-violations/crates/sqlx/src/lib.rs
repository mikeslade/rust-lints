//! Stand-in for the `sqlx` facade: the same re-exports as the real crate.

pub use sqlx_core::AssertSqlSafe;
pub use sqlx_core::query::query;
pub use sqlx_core::query::query_with_result as __query_with_result;
pub use sqlx_core::query_as::query_as;
pub use sqlx_core::query_scalar::query_scalar;

pub struct Postgres;

/// The compile-time checked macro. Like the real one, it expands to a private entry
/// point rather than to the runtime constructors.
#[macro_export]
macro_rules! query {
    ($sql:literal) => {
        $crate::__query_with_result::<$crate::Postgres>($sql)
    };
}
