//! Which calls are SQLx runtime query constructors, and which of them carry static SQL.
//!
//! The strict compile-time-SQLx pass and the dynamic-SQL safety pass both start from a
//! call to `sqlx::query`, `sqlx::query_as` or `sqlx::query_scalar`. They used to decide
//! that from the callee's source text, and the text missed two shapes that are the same
//! call (techzenlabs/radiology-platform#1932):
//!
//! - **A turbofish.** `sqlx::query_scalar::<_, String>("..")` has the callee text
//!   `sqlx::query_scalar::<_, String>`, which neither equals `sqlx::query_scalar` nor
//!   contains `sqlx::query_scalar(`, so both passes skipped the call.
//! - **SQL that is not a literal at the call site.** `sqlx::query(FACT_SQL)` passed the
//!   callee test, but the strict pass only recognised a string literal written inside
//!   the parentheses, so moving the query text into a `const` took it out of scope.
//!
//! # How a call is judged
//!
//! - **The callee** is the function the path resolved to, so a turbofish, a `use` of the
//!   function under another name, and the `sqlx` facade's re-export all reach the same
//!   definition in `sqlx_core`. A local function or module that is merely called `query`
//!   or `sqlx` is not SQLx.
//! - **The SQL argument** is followed to its definition through HIR and the const
//!   evaluator: a string literal (including what `concat!` produced), a `const` or
//!   associated const anywhere, a `static` in this crate, a reference to any of those,
//!   and a call to a zero-argument function in this crate whose body is one of those.
//!   A `const` or `static` whose text cannot be read is still static SQL: it is reported
//!   rather than waved through.
//!
//! The dynamic-SQL pass still reads the call's source text for its argument heuristics,
//! but the text it reads has the turbofish removed, so a turbofished call gets the same
//! verdict as the plain one.

use rustc_ast::LitKind;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind, QPath};
use rustc_lint::LateContext;
use rustc_middle::ty::TyCtxt;
use rustc_span::def_id::DefId;

use crate::snippet;

/// How far a const, static or function body is followed before the pass gives up.
/// Rust rejects cyclic consts, so this only bounds a long chain of aliases.
const MAX_RESOLUTION_DEPTH: usize = 16;

/// The crate that defines SQLx's runtime query constructors. The `sqlx` crate
/// re-exports them, and a re-export resolves to the original definition.
const SQLX_CORE_CRATE: &str = "sqlx_core";

/// The module path, inside [`SQLX_CORE_CRATE`], of each runtime query constructor.
const SQLX_RUNTIME_QUERY_PATHS: &[&[&str]] = &[
    &["query", "query"],
    &["query_as", "query_as"],
    &["query_scalar", "query_scalar"],
];

/// SQL text whose value is fixed at compile time.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StaticSql {
    /// The text, read from a literal or from the evaluated constant.
    Text(String),
    /// A `const` or `static` whose text could not be read. It is still static.
    Unreadable,
}

/// The function a call's callee path resolved to, when it is a path.
fn callee_def_id(cx: &LateContext<'_>, callee: &Expr<'_>) -> Option<DefId> {
    let ExprKind::Path(qpath) = callee.kind else {
        return None;
    };
    cx.qpath_res(&qpath, callee.hir_id).opt_def_id()
}

/// True when `callee` resolves to one of SQLx's runtime query constructors.
pub(crate) fn is_sqlx_runtime_query_callee(cx: &LateContext<'_>, callee: &Expr<'_>) -> bool {
    callee_def_id(cx, callee).is_some_and(|def_id| is_sqlx_runtime_query_fn(cx.tcx, def_id))
}

fn is_sqlx_runtime_query_fn(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    if !matches!(tcx.def_kind(def_id), DefKind::Fn) {
        return false;
    }
    let crate_name = tcx.crate_name(def_id.krate);
    let path: Vec<String> = tcx
        .def_path(def_id)
        .data
        .iter()
        .map(|component| component.data.to_string())
        .collect();
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    is_sqlx_runtime_query_path(crate_name.as_str(), &path)
}

/// The pure half of [`is_sqlx_runtime_query_fn`]: a crate name and the definition's
/// module path within it.
pub(crate) fn is_sqlx_runtime_query_path(crate_name: &str, path: &[&str]) -> bool {
    crate_name == SQLX_CORE_CRATE && SQLX_RUNTIME_QUERY_PATHS.contains(&path)
}

/// The call's source text with the callee's turbofish removed, so the dynamic-SQL
/// heuristics see `sqlx::query_as(sql.as_str())` for `sqlx::query_as::<_, T>(sql.as_str())`.
///
/// The cut is made at the end of the callee path's last segment identifier, a position
/// HIR records, not at a searched-for `::<`.
pub(crate) fn call_text_without_turbofish(
    cx: &LateContext<'_>,
    call: &Expr<'_>,
    callee: &Expr<'_>,
) -> String {
    let Some(ident_end) = last_segment_ident_end(callee) else {
        return snippet(cx, call.span);
    };
    if !call.span.contains(callee.span) || !callee.span.contains(ident_end) {
        return snippet(cx, call.span);
    }
    let head = snippet(cx, callee.span.with_hi(ident_end.hi()));
    let tail = snippet(cx, call.span.with_lo(callee.span.hi()));
    format!("{head}{tail}")
}

fn last_segment_ident_end(callee: &Expr<'_>) -> Option<rustc_span::Span> {
    let ExprKind::Path(qpath) = callee.kind else {
        return None;
    };
    let ident = match qpath {
        QPath::Resolved(_, path) => path.segments.last()?.ident,
        QPath::TypeRelative(_, segment) => segment.ident,
    };
    Some(ident.span.shrink_to_hi())
}

/// The static SQL an argument carries, followed to its definition, or `None` when the
/// argument is not provably fixed at compile time (a local variable, a `format!`, an
/// `AssertSqlSafe(..)` wrapper, a call into another crate).
pub(crate) fn static_sql_argument<'tcx>(
    cx: &LateContext<'tcx>,
    arg: &Expr<'tcx>,
) -> Option<StaticSql> {
    let ty = cx.typeck_results().expr_ty(arg);
    if !ty.peel_refs().is_str() {
        return None;
    }
    static_sql_of(cx.tcx, arg, 0)
}

fn static_sql_of<'tcx>(tcx: TyCtxt<'tcx>, expr: &Expr<'tcx>, depth: usize) -> Option<StaticSql> {
    if depth > MAX_RESOLUTION_DEPTH {
        return Some(StaticSql::Unreadable);
    }
    match expr.kind {
        ExprKind::Lit(lit) => match lit.node {
            LitKind::Str(symbol, _) => Some(StaticSql::Text(symbol.as_str().to_owned())),
            _ => None,
        },
        ExprKind::AddrOf(_, _, inner) | ExprKind::DropTemps(inner) => {
            static_sql_of(tcx, inner, depth + 1)
        }
        ExprKind::Block(block, _) if block.stmts.is_empty() => {
            static_sql_of(tcx, block.expr?, depth + 1)
        }
        ExprKind::Path(qpath) => {
            let res = path_res(tcx, expr, &qpath)?;
            static_sql_of_item(tcx, res, depth)
        }
        ExprKind::Call(callee, []) => {
            let ExprKind::Path(qpath) = callee.kind else {
                return None;
            };
            let Res::Def(DefKind::Fn | DefKind::AssocFn, def_id) = path_res(tcx, callee, &qpath)?
            else {
                return None;
            };
            let body = tcx.hir_maybe_body_owned_by(def_id.as_local()?)?;
            static_sql_of(tcx, body.value, depth + 1)
        }
        _ => None,
    }
}

/// What a path expression resolved to. A type-relative path (`Self::SQL`) is resolved
/// through the type-check results of the body it appears in.
fn path_res<'tcx>(tcx: TyCtxt<'tcx>, expr: &Expr<'tcx>, qpath: &QPath<'tcx>) -> Option<Res> {
    match qpath {
        QPath::Resolved(_, path) => Some(path.res),
        QPath::TypeRelative(..) => tcx
            .typeck(tcx.hir_enclosing_body_owner(expr.hir_id))
            .type_dependent_def(expr.hir_id)
            .map(|(kind, def_id)| Res::Def(kind, def_id)),
    }
}

fn static_sql_of_item(tcx: TyCtxt<'_>, res: Res, depth: usize) -> Option<StaticSql> {
    match res {
        Res::Def(DefKind::Const | DefKind::AssocConst, def_id) => {
            Some(evaluated_const_text(tcx, def_id).map_or(StaticSql::Unreadable, StaticSql::Text))
        }
        Res::Def(DefKind::Static { .. }, def_id) => {
            let Some(local) = def_id.as_local() else {
                return Some(StaticSql::Unreadable);
            };
            let Some(body) = tcx.hir_maybe_body_owned_by(local) else {
                return Some(StaticSql::Unreadable);
            };
            Some(static_sql_of(tcx, body.value, depth + 1).unwrap_or(StaticSql::Unreadable))
        }
        _ => None,
    }
}

/// The text a `&str` constant evaluates to, wherever it is defined.
fn evaluated_const_text(tcx: TyCtxt<'_>, def_id: DefId) -> Option<String> {
    let value = tcx.const_eval_poly(def_id).ok()?;
    let bytes = value.try_get_slice_bytes_for_diagnostics(tcx)?;
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

/// True when static SQL text reads as a DML statement the compile-time macros can check.
/// Whitespace is collapsed first, so `SELECT` at the end of a line counts the same as
/// `SELECT ` mid-line; a query moved into a multi-line `const` is the usual case.
pub(crate) fn is_static_dml(sql: &StaticSql) -> bool {
    let StaticSql::Text(text) = sql else {
        return true;
    };
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let normalized = format!("{normalized} ");
    ["select ", "insert into", "update ", "delete from"]
        .iter()
        .any(|keyword| normalized.contains(keyword))
}

#[cfg(test)]
mod tests {
    use super::{StaticSql, is_sqlx_runtime_query_path, is_static_dml};

    #[test]
    fn identifies_runtime_query_constructors_by_crate_and_path() {
        assert!(is_sqlx_runtime_query_path("sqlx_core", &["query", "query"]));
        assert!(is_sqlx_runtime_query_path(
            "sqlx_core",
            &["query_as", "query_as"]
        ));
        assert!(is_sqlx_runtime_query_path(
            "sqlx_core",
            &["query_scalar", "query_scalar"]
        ));
        // The macros' private entry point, the `_with` variants and the builder are not
        // the constructors these passes audit.
        assert!(!is_sqlx_runtime_query_path(
            "sqlx_core",
            &["query", "query_with_result"]
        ));
        assert!(!is_sqlx_runtime_query_path(
            "sqlx_core",
            &["query", "query_with"]
        ));
        // A same-named function anywhere else is not SQLx.
        assert!(!is_sqlx_runtime_query_path("my_app", &["sqlx", "query"]));
        assert!(!is_sqlx_runtime_query_path("sqlx", &["query"]));
    }

    #[test]
    fn static_dml_ignores_whitespace_shape() {
        assert!(is_static_dml(&StaticSql::Text(
            "--sql\nSELECT\n  id\nFROM workers".to_owned()
        )));
        assert!(is_static_dml(&StaticSql::Text(
            "delete\n from x".to_owned()
        )));
        assert!(is_static_dml(&StaticSql::Text(
            "insert\tinto x values (1)".to_owned()
        )));
        assert!(!is_static_dml(&StaticSql::Text(
            "SET LOCAL lock_timeout = '1s'".to_owned()
        )));
        // Unreadable static text fails closed.
        assert!(is_static_dml(&StaticSql::Unreadable));
    }
}
