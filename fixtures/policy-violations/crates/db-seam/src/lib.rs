//! Fixtures for the SQLx runtime-query passes.
//!
//! `scripts/check-sqlx-query-fixture.sh` lints this crate as the SQL seam
//! (`RUST_LINTS_SQLX_OWNER_PATHS=crates/db-seam/`) with `RUST_LINTS_STRICT_SQLX=1`, and
//! set-diffs what the two passes report against the trailing markers:
//!
//! - `// sqlx-query: expect-static`   static SQL handed to a runtime constructor
//! - `// sqlx-query: expect-dynamic`  dynamic SQL with no documented safety boundary
//!
//! `sqlx` and `sqlx_core` here are the stand-in crates next to this one, with the real
//! crates' names and item paths.

use sqlx::{AssertSqlSafe, Postgres};
use sqlx::query_as as fetch_rows;

pub struct Row;

// ---------------------------------------------------------------------------
// Static SQL: an inline literal.
// ---------------------------------------------------------------------------

pub fn static_query_without_macro() {
    let _: sqlx_core::Query<Postgres> = sqlx::query("SELECT id FROM workers"); // sqlx-query: expect-static
}

pub fn static_scalar_without_macro() {
    sqlx::query_scalar::<Postgres, i64>("SELECT COUNT(*) FROM elections"); // sqlx-query: expect-static
}

// #1932's turbofish repro: the callee text is `sqlx::query_scalar::<_, String>`.
pub fn turbofished_scalar() {
    let _: sqlx_core::QueryScalar<Postgres, String> = sqlx::query_scalar::<_, String>( // sqlx-query: expect-static
        "--sql
SELECT name FROM workers WHERE id = $1",
    );
}

// The keyword is followed by a newline, not a space.
pub fn multi_line_literal() {
    let _: sqlx_core::Query<Postgres> = sqlx::query( // sqlx-query: expect-static
        "--sql
SELECT
    id
FROM workers",
    );
}

// ---------------------------------------------------------------------------
// Static SQL that is not a literal at the call site (#1932's const-sourced half).
// ---------------------------------------------------------------------------

const FACT_SQL: &str = "--sql
SELECT id, status
FROM facts
WHERE id = $1";

const ALIAS_SQL: &str = FACT_SQL;

const CONCAT_SQL: &str = concat!("--sql\n", "DELETE FROM facts WHERE id = $1");

static STATIC_SQL: &str = "--sql
UPDATE facts SET status = $2 WHERE id = $1";

mod queries {
    pub const NESTED_SQL: &str = "--sql
INSERT INTO facts (id) VALUES ($1)";
}

const fn derive_provider_sql() -> &'static str {
    "--sql
select r.id from referrers r where r.code = $1"
}

impl Row {
    const SQL: &str = "--sql
SELECT id FROM rows";

    pub fn assoc_const_via_self() {
        let _: sqlx_core::Query<Postgres> = sqlx::query(Self::SQL); // sqlx-query: expect-static
    }
}

pub fn const_sourced() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(FACT_SQL); // sqlx-query: expect-static
}

pub fn turbofished_const_sourced() {
    let _: sqlx_core::QueryAs<Postgres, Row> = sqlx::query_as::<_, Row>(FACT_SQL); // sqlx-query: expect-static
}

pub fn single_turbofish_const_sourced() {
    let _: sqlx_core::Query<Postgres> = sqlx::query::<_>(FACT_SQL); // sqlx-query: expect-static
}

pub fn const_in_another_module() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(queries::NESTED_SQL); // sqlx-query: expect-static
}

pub fn const_alias() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(ALIAS_SQL); // sqlx-query: expect-static
}

pub fn concat_const() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(CONCAT_SQL); // sqlx-query: expect-static
}

pub fn static_item() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(STATIC_SQL); // sqlx-query: expect-static
}

pub fn borrowed_const() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(&FACT_SQL); // sqlx-query: expect-static
}

pub fn assoc_const_via_type() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(Row::SQL); // sqlx-query: expect-static
}

pub fn const_fn_body() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(derive_provider_sql()); // sqlx-query: expect-static
}

pub fn const_in_another_crate() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(fixture_sql_consts::FOREIGN_SQL); // sqlx-query: expect-static
}

// A `static` in another crate has no readable HIR here. It is still static SQL, so it is
// reported rather than waved through.
pub fn static_in_another_crate() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(fixture_sql_consts::FOREIGN_STATIC_SQL); // sqlx-query: expect-static
}

// A trait's associated const is evaluated for the implementation the path reaches. The
// path itself names the trait's declaration, which has no value (a required const) or
// the wrong one (an overridden default).
pub trait SqlSource {
    const SQL: &'static str;
}

pub trait SqlWithDefault {
    const SQL: &'static str = "SET LOCAL lock_timeout = '1s'";
}

pub struct Ledger;
pub struct Journal;
pub struct Settings;

impl SqlSource for Ledger {
    const SQL: &'static str = "--sql
SELECT id FROM ledger";
}

impl SqlWithDefault for Journal {
    const SQL: &'static str = "--sql
SELECT id FROM journal";
}

impl SqlWithDefault for Settings {}

pub fn trait_const_required() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(Ledger::SQL); // sqlx-query: expect-static
    let _: sqlx_core::Query<Postgres> = sqlx::query(<Ledger as SqlSource>::SQL); // sqlx-query: expect-static
}

pub fn trait_const_overridden_default() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(Journal::SQL); // sqlx-query: expect-static
}

// Still generic here, so its text cannot be read; it is static all the same.
pub fn trait_const_generic<T: SqlSource>() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(T::SQL); // sqlx-query: expect-static
}

// A `use` under another name reaches the same definition.
pub fn renamed_import() {
    let _: sqlx_core::QueryAs<Postgres, Row> = fetch_rows::<_, Row>(FACT_SQL); // sqlx-query: expect-static
}

// Calling `sqlx_core` directly, without the facade.
pub fn core_path() {
    let _: sqlx_core::Query<Postgres> = sqlx_core::query::query(FACT_SQL); // sqlx-query: expect-static
}

// ---------------------------------------------------------------------------
// Dynamic SQL with no documented safety boundary.
// ---------------------------------------------------------------------------

pub fn dynamic_query_without_safety_note(table: &str) {
    let sql = format!("select * from {table}");
    let _: sqlx_core::Query<Postgres> = sqlx::query(sql.as_str()); // sqlx-query: expect-dynamic
}

// The turbofish used to take the call out of this pass entirely.
pub fn turbofished_dynamic_query(table: &str) {
    let sql = format!("select * from {table}");
    let _: sqlx_core::QueryAs<Postgres, Row> = sqlx::query_as::<_, Row>(sql.as_str()); // sqlx-query: expect-dynamic
}

// The dynamic pass matches `query(&sql)` on the call's text, so it only sees this call
// once the turbofish is cut out of that text.
pub fn turbofished_borrowed_sql(table: &str) {
    let sql = format!("select * from {table}");
    let _: sqlx_core::Query<Postgres> = sqlx::query::<_>(&sql); // sqlx-query: expect-dynamic
}

pub fn turbofished_dynamic_scalar(table: &str) {
    let _: sqlx_core::QueryScalar<Postgres, i64> =
        sqlx::query_scalar::<_, i64>(format!("select count(*) from {table}")); // sqlx-query: expect-dynamic
}

// ---------------------------------------------------------------------------
// CLEAN: forms both passes allow.
// ---------------------------------------------------------------------------

// The compile-time checked macro.
pub fn checked_macro() {
    sqlx::query!("SELECT id FROM workers");
}

// Dynamic SQL across the documented boundary, plain and turbofished.
pub fn asserted_dynamic(table: &str) {
    let _: sqlx_core::Query<Postgres> =
        sqlx::query(AssertSqlSafe(format!("select * from {table}")));
    let sql = format!("select * from {table}");
    let _: sqlx_core::QueryAs<Postgres, Row> = sqlx::query_as::<_, Row>(AssertSqlSafe(sql));
}

// Dynamic SQL with a documented safety note.
pub fn noted_dynamic(table: &str) {
    let sql = format!("select * from {table}");
    // rust-lints-dynamic-sql: the table name comes from a closed enum.
    let _: sqlx_core::Query<Postgres> = sqlx::query(sql.as_str());
}

// Static text that is not a statement the macros check.
const LOCK_TIMEOUT_SQL: &str = "SET LOCAL lock_timeout = '1s'";

pub fn static_non_dml() {
    let _: sqlx_core::Query<Postgres> = sqlx::query(LOCK_TIMEOUT_SQL);
    let _: sqlx_core::Query<Postgres> = sqlx::query("SET LOCAL statement_timeout = '5s'");
    // The trait's non-DML default, which `Settings` does not override.
    let _: sqlx_core::Query<Postgres> = sqlx::query(Settings::SQL);
    // A trait method's path names its declaration; the default body below is not what
    // `Settings` runs, so it proves nothing.
    let _: sqlx_core::Query<Postgres> = sqlx::query(Settings::sql());
}

pub trait SqlFn {
    fn sql() -> &'static str {
        "--sql
SELECT id FROM defaults"
    }
}

impl SqlFn for Settings {
    fn sql() -> &'static str {
        "SET LOCAL lock_timeout = '1s'"
    }
}

// A function or module that is merely called `query` or `sqlx` is not SQLx.
mod shadow {
    pub mod sqlx {
        pub fn query(_sql: &str) {}
    }

    pub fn query(_sql: &str) {}
}

pub fn same_name_elsewhere() {
    shadow::sqlx::query("--sql SELECT id FROM shadows");
    shadow::query(FACT_SQL);
}

pub fn inline_sql_without_marker() -> &'static str {
    "SELECT id FROM deductions"
}
