//! Stand-in for `sqlx_core`. Only the item paths matter: `query::query`,
//! `query_as::query_as` and `query_scalar::query_scalar` are where the real crate
//! defines the runtime query constructors that `sqlx` re-exports.

use std::marker::PhantomData;

pub struct Query<DB>(PhantomData<DB>);
pub struct QueryAs<DB, O>(PhantomData<(DB, O)>);
pub struct QueryScalar<DB, O>(PhantomData<(DB, O)>);

pub struct AssertSqlSafe<T>(pub T);

pub mod query {
    use super::{PhantomData, Query};

    pub fn query<DB>(_sql: impl Sized) -> Query<DB> {
        Query(PhantomData)
    }

    /// What the compile-time `query!` macro expands to in the real crate.
    pub fn query_with_result<DB>(_sql: impl Sized) -> Query<DB> {
        Query(PhantomData)
    }
}

pub mod query_as {
    use super::{PhantomData, QueryAs};

    pub fn query_as<DB, O>(_sql: impl Sized) -> QueryAs<DB, O> {
        QueryAs(PhantomData)
    }
}

pub mod query_scalar {
    use super::{PhantomData, QueryScalar};

    pub fn query_scalar<DB, O>(_sql: impl Sized) -> QueryScalar<DB, O> {
        QueryScalar(PhantomData)
    }
}
