//! Pass: build-time `CARGO_MANIFEST_DIR` read (`RUST_LINTS_BUILD_TIME_MANIFEST_DIR`).
//!
//! `env!("CARGO_MANIFEST_DIR")` and `option_env!("CARGO_MANIFEST_DIR")` are fixed when
//! rustc compiles the crate: the value names the checkout the binary was BUILT in. When a
//! target dir is shared between checkouts, cargo reuses a binary built in checkout B as
//! fresh for checkout A, and a test that builds a path from that value reads B's tree.
//! The run-time `std::env::var_os("CARGO_MANIFEST_DIR")` that cargo exports to the test
//! process has no such problem, and this pass leaves it alone.
//!
//! # What the pass judges
//!
//! Every judgement is made after macro expansion and name resolution, so neither the
//! spelling of the macro nor the spelling of its argument matters:
//!
//! - **Which macro.** An expansion is a build-time read when the macro it expanded
//!   RESOLVED to `core`'s builtin `env` or `option_env` (the `env_macro` and
//!   `option_env_macro` diagnostic items). `use std::env as r; r!(..)`, `core::env!`, and
//!   a `use` that a `macro_rules!` assembled from an `ident`, a `path` or forwarded
//!   token trees all resolve to the same definition.
//! - **Which variable.** rustc records every variable an `env!`/`option_env!` read, with
//!   the value it got, in the session's env dep-info (it feeds the `.d` file cargo uses to
//!   rebuild when a variable changes). The record is made after the builtin has expanded
//!   its argument, so `r!(name!())` is recorded under the name `name!()` built. A crate
//!   whose record has no `CARGO_MANIFEST_DIR` entry read it nowhere, and the pass stops.
//! - **Which site.** The builtin replaces the call with a string literal carrying the
//!   variable's value, marked with the call's expansion. A call whose literal survives
//!   into HIR read `CARGO_MANIFEST_DIR` exactly when the literal equals the value that
//!   variable has in the record. A call whose literal another builtin consumed
//!   (`concat!(env!(..), "/x")`, `include_str!(concat!(env!(..), ..))`) leaves nothing in
//!   HIR to compare, so in a crate that does read the variable the pass refuses it too:
//!   it cannot rule the read out. That is the only place it reports a call it has not
//!   proven, and it can only happen in a crate that is already refused.
//!
//! The consumed calls are found by walking every expansion the crate made, which rustc
//! exposes only through [`debug_hygiene_data`]. That walk runs only in a crate the
//! dep-info shows reading the variable, and it fails closed: an expansion table the pass
//! cannot read is itself a finding, never a clean crate.
//!
//! # Scope
//!
//! No path, target or test exemption. A read in a test, bench, example or build script
//! bakes the checkout just the same, and the guard this pass backs (radiology-platform
//! #2656, `rk-lite manifest-dir`) has none either.

use std::collections::BTreeMap;

use rustc_ast::LitKind;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::{LateContext, LintContext};
use rustc_span::hygiene::{ExpnData, LocalExpnId, debug_hygiene_data};
use rustc_span::{DUMMY_SP, Span, Symbol};

use crate::{config, emit};

/// The variable whose build-time value names the checkout the crate was compiled in.
pub(crate) const VARIABLE: &str = "CARGO_MANIFEST_DIR";

/// The diagnostic items `core` puts on its builtin `env!` and `option_env!`.
const ENV_MACRO_ITEMS: &[&str] = &["env_macro", "option_env_macro"];

/// How rustc's hygiene table names an expansion of the crate being compiled.
const LOCAL_EXPANSION_PREFIX: &str = "crate0::{{expn";

/// What HIR still shows of one `env!`/`option_env!` expansion.
#[derive(Default)]
struct Output {
    /// The string literal the builtin produced, when it survived.
    literal: Option<Symbol>,
}

/// Per-crate state, filled by [`ManifestDirReads::observe_expr`] and judged by
/// [`ManifestDirReads::check_crate_post`].
#[derive(Default)]
pub(crate) struct ManifestDirReads {
    /// The value the env dep-info records for [`VARIABLE`]: `None` until the first
    /// expression is seen, `Some(None)` when the crate never read it.
    recorded: Option<Option<Option<Symbol>>>,
    /// Every `env!`/`option_env!` expansion with output left in HIR, by local index.
    outputs: BTreeMap<u32, Output>,
}

impl ManifestDirReads {
    /// The value recorded for [`VARIABLE`], or `None` when this crate never read it at
    /// build time. The inner `Option` is the value itself (`None`: read while unset).
    fn recorded(&mut self, cx: &LateContext<'_>) -> Option<Option<Symbol>> {
        *self.recorded.get_or_insert_with(|| recorded_value(cx))
    }

    /// Record `expr` when it is (part of) an `env!`/`option_env!` expansion's output.
    pub(crate) fn observe_expr(&mut self, cx: &LateContext<'_>, expr: &Expr<'_>) {
        if !config::build_time_manifest_dir_enabled() || self.recorded(cx).is_none() {
            return;
        }
        let ctxt = expr.span.ctxt();
        if ctxt.is_root() {
            return;
        }
        let Some(expn) = ctxt.outer_expn().as_local() else {
            return;
        };
        if !is_env_macro_expansion(cx, &expn.expn_data()) {
            return;
        }
        let output = self.outputs.entry(expn.as_u32()).or_default();
        if let ExprKind::Lit(lit) = expr.kind
            && let LitKind::Str(value, _) = lit.node
        {
            output.literal = Some(value);
        }
    }

    pub(crate) fn check_crate_post(&mut self, cx: &LateContext<'_>) {
        if !config::build_time_manifest_dir_enabled() {
            return;
        }
        let Some(recorded) = self.recorded(cx) else {
            return;
        };
        // An expansion table with unset slots exists only when expansion itself failed,
        // and reading it would panic. The crate does not build, so nothing is lost.
        if cx.sess().dcx().has_errors().is_some() {
            return;
        }
        let Some(expansions) = local_expansions() else {
            emit(
                cx,
                DUMMY_SP,
                format!("could not locate this crate's build-time `{VARIABLE}` read"),
                "the crate reads it through `env!`/`option_env!`, but rustc's expansion table did not parse; the table's format changed under this nightly, so update the build-time manifest-dir pass in rust-lints".to_owned(),
            );
            return;
        };

        for expn in expansions {
            let data = expn.expn_data();
            if !is_env_macro_expansion(cx, &data) {
                continue;
            }
            let span = reporting_span(cx, data.call_site);
            match self.outputs.get(&expn.as_u32()) {
                // `option_env!` of an unset variable leaves a `None` path and no literal,
                // which matches only a recorded read of an unset `CARGO_MANIFEST_DIR`.
                Some(output) if output.literal == recorded => emit(
                    cx,
                    span,
                    format!("`env!`/`option_env!` must not read `{VARIABLE}` at build time"),
                    format!(
                        "the value names the checkout this crate was compiled in, and a shared target dir reuses the binary in another checkout; read `std::env::var_os(\"{VARIABLE}\")` at run time and fail when it is unset, and give `include_str!`/`include_bytes!` a path relative to the source file"
                    ),
                ),
                Some(_) => {}
                None => emit(
                    cx,
                    span,
                    format!(
                        "`env!`/`option_env!` consumed by another macro in a crate that reads \
                         `{VARIABLE}` at build time"
                    ),
                    format!(
                        "its value went into another macro (`concat!`, `include_str!`, an attribute), so the lint cannot rule out that this call is the read; read `std::env::var_os(\"{VARIABLE}\")` at run time instead, and give `include_str!`/`include_bytes!` a path relative to the source file"
                    ),
                ),
            }
        }
    }
}

/// The value the session's env dep-info recorded for [`VARIABLE`], or `None` when no
/// `env!`/`option_env!` (or tracked proc-macro read) in this crate named it.
fn recorded_value(cx: &LateContext<'_>) -> Option<Option<Symbol>> {
    cx.sess()
        .env_depinfo
        .borrow()
        .iter()
        .find(|(name, _)| name.as_str() == VARIABLE)
        .map(|(_, value)| *value)
}

/// True when `data` is an expansion of the macro `core` defines as the builtin `env` or
/// `option_env`, however the call spelled or reached it.
fn is_env_macro_expansion(cx: &LateContext<'_>, data: &ExpnData) -> bool {
    data.macro_def_id
        .and_then(|def_id| cx.tcx.get_diagnostic_name(def_id))
        .is_some_and(|name| ENV_MACRO_ITEMS.contains(&name.as_str()))
}

/// Where to report a call: its own site, unless that sits inside a macro another crate
/// defined, where rustc would drop the lint. The outermost call in this crate is reported
/// then, so a macro exported by a sibling crate cannot hide the read.
fn reporting_span(cx: &LateContext<'_>, call_site: Span) -> Span {
    if call_site.in_external_macro(cx.sess().source_map()) {
        call_site.source_callsite()
    } else {
        call_site
    }
}

/// Every expansion this crate made, root excluded, or `None` when rustc's table cannot be
/// read. Bounds come from [`debug_hygiene_data`], the one public view of the table.
fn local_expansions() -> Option<Vec<LocalExpnId>> {
    let count = local_expansion_count(&debug_hygiene_data(false))?;
    Some((1..count).map(LocalExpnId::from_u32).collect())
}

/// The number of local expansions in a [`debug_hygiene_data`] dump, root included.
///
/// The dump lists the local expansions first, one line each, numbered from 0. Anything
/// other than one unbroken run `0, 1, .., n - 1` means the format is not the one this
/// parser knows, and the answer is `None`, never a short count.
fn local_expansion_count(dump: &str) -> Option<u32> {
    let mut count: u32 = 0;
    for line in dump.lines() {
        let Some(rest) = line.strip_prefix(LOCAL_EXPANSION_PREFIX) else {
            continue;
        };
        let index: u32 = rest.split_once("}}:")?.0.parse().ok()?;
        if index != count {
            return None;
        }
        count = count.checked_add(1)?;
    }
    (count > 0).then_some(count)
}

#[cfg(test)]
mod tests {
    use super::local_expansion_count;

    #[test]
    fn counts_the_local_expansions_of_a_hygiene_dump() {
        let dump = "Expansions:\n\
            crate0::{{expn0}}: parent: crate0::{{expn0}}, call_site_ctxt: #0, def_site_ctxt: #0, kind: Root\n\
            crate0::{{expn1}}: parent: crate0::{{expn0}}, call_site_ctxt: #0, def_site_ctxt: #0, kind: AstPass(StdImports)\n\
            crate0::{{expn2}}: parent: crate0::{{expn0}}, call_site_ctxt: #0, def_site_ctxt: #0, kind: Macro(Bang, \"env\")\n\
            crate1::{{expn1}}: parent: crate0::{{expn0}}, call_site_ctxt: #0, def_site_ctxt: #0, kind: AstPass(StdImports)\n\
            \n\
            SyntaxContexts:\n\
            #0: parent: #0, outer_mark: (crate0::{{expn0}}, Opaque)\n";
        assert_eq!(local_expansion_count(dump), Some(3));
    }

    #[test]
    fn refuses_a_dump_it_cannot_read() {
        // No local line at all: a changed format, not a crate with no expansions (the
        // root expansion always exists).
        assert_eq!(
            local_expansion_count("Expansions:\nExpnId(0): Root\n"),
            None
        );
        // A gap or a reordering would make the walk skip expansions.
        assert_eq!(
            local_expansion_count("crate0::{{expn0}}: kind: Root\ncrate0::{{expn2}}: kind: Root\n"),
            None
        );
        // An index that does not parse.
        assert_eq!(
            local_expansion_count("crate0::{{expnX}}: kind: Root\n"),
            None
        );
    }
}
