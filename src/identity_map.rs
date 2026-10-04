//! Structural proof that a mapping function returns its argument unchanged.
//!
//! The silent-saturation pass refuses `T::try_from(x).unwrap_or(MAX)`. With an identity
//! mapping, `map_or(MAX, |v| v)` and `map_or_else(|_| MAX, |v| v)` are the same
//! expression under another name, and clippy's `option_if_let_else` suggests exactly that
//! rewrite (techzenlabs/radiology-platform#1934). This module decides when the mapping is
//! the identity, from HIR and types rather than from source text:
//!
//! - a path that resolves to `core::convert::identity` (the `convert_identity` diagnostic
//!   item), however it is spelled or imported;
//! - a non-async closure with one by-value binding parameter whose body, after any
//!   statement-free blocks, is that binding, or that binding cast to the type it already
//!   has (`|v| v as u32` when `v: u32`).
//!
//! A body that does anything else is not proven to be the identity, and the pass stays
//! quiet about it, as it always did for `map_or`.

use rustc_hir::def::Res;
use rustc_hir::{BindingMode, ByRef, ClosureKind, Expr, ExprKind, HirId, PatKind, QPath};
use rustc_lint::LateContext;
use rustc_span::Symbol;

/// The diagnostic item `core` puts on `core::convert::identity`. It is not one of
/// rustc's pre-interned symbols, so it is interned on use.
const IDENTITY_FN_ITEM: &str = "convert_identity";

/// True when `mapping` provably returns its one argument unchanged.
pub(crate) fn is_identity_mapping(cx: &LateContext<'_>, mapping: &Expr<'_>) -> bool {
    match mapping.kind {
        ExprKind::Path(qpath) => cx
            .qpath_res(&qpath, mapping.hir_id)
            .opt_def_id()
            .is_some_and(|def_id| {
                cx.tcx
                    .is_diagnostic_item(Symbol::intern(IDENTITY_FN_ITEM), def_id)
            }),
        ExprKind::Closure(closure) => {
            if !matches!(closure.kind, ClosureKind::Closure) {
                return false;
            }
            let body = cx.tcx.hir_body(closure.body);
            let [param] = body.params else {
                return false;
            };
            let PatKind::Binding(BindingMode(ByRef::No, _), binding, _, None) = param.pat.kind
            else {
                return false;
            };
            returns_binding(cx, body.value, binding)
        }
        _ => false,
    }
}

/// True when `expr` evaluates to the local `binding` with its value and type unchanged.
fn returns_binding(cx: &LateContext<'_>, expr: &Expr<'_>, binding: HirId) -> bool {
    match expr.kind {
        ExprKind::Block(block, _) if block.stmts.is_empty() => block
            .expr
            .is_some_and(|tail| returns_binding(cx, tail, binding)),
        ExprKind::Path(QPath::Resolved(None, path)) => path.res == Res::Local(binding),
        ExprKind::Cast(inner, _) => {
            let typeck = cx.typeck_results();
            typeck.expr_ty(inner) == typeck.expr_ty(expr) && returns_binding(cx, inner, binding)
        }
        _ => false,
    }
}
