#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! Shared architecture-policy Dylint.
//!
//! This crate is the generic union of policy lints that several production
//! Rust workspaces vendored as near-identical copies. A single lint,
//! [`RUST_LINTS_POLICY_CHECKS`], bundles the shared policy passes:
//!
//! 1. SQL seam: SQLx / database-adapter APIs must stay behind reviewed db crates.
//! 2. HTTP wrapper: outbound HTTP must go through reviewed wrapper crates.
//! 3. Blocking-in-async: no blocking calls on Tokio async worker threads.
//! 4. File-length ratchet: source files must stay under a configured line cap
//!    unless a documented exception (inline marker or TSV inventory) covers them.
//! 5. Silent numeric saturation: a fallible numeric conversion whose error is
//!    swallowed by the `unwrap_or` family silently corrupts a persisted value.
//! 6. Unbounded channel: a channel constructor with no backpressure bound.
//! 7. Boolean parameter on a public fn: blind at call sites; use a named enum.
//!
//! Everything that used to be hardcoded per project (seam crate names, fixture
//! path prefix, repo source roots, exception-marker keyword, message wording) is
//! driven by `RUST_LINTS_*` environment variables with sensible defaults. See the
//! [`config`] module for the full contract.

extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rustc_errors::{Diag, Diagnostic, EmissionGuarantee, Level, MultiSpan};
use rustc_hir::def_id::LocalDefId;
use rustc_hir::intravisit::FnKind;
use rustc_hir::{
    AmbigArg, Body, Closure, ClosureKind, CoroutineDesugaring, CoroutineKind, Expr, ExprKind,
    FnDecl, Item, ItemKind, Node, Ty, TyKind, UseKind,
};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::ty;
use rustc_span::{DUMMY_SP, FileName, Span};

dylint_linting::declare_late_lint! {
    /// ### What it does
    ///
    /// Enforces repository architecture policy shared across consuming projects.
    ///
    /// ### Why is this bad?
    ///
    /// Projects keep SQL, outbound HTTP clients, and blocking runtime
    /// integrations behind reviewed infrastructure boundaries so product crates
    /// do not absorb database adapters, PII/PHI-sensitive network behavior, or
    /// async scheduler hazards. Source files are also kept small enough to parse
    /// and review.
    pub RUST_LINTS_POLICY_CHECKS,
    Warn,
    "enforces shared Rust architecture policy checks"
}

/// Generic configuration contract.
///
/// Every knob is an environment variable read at lint time. Feature gates
/// (`HTTP_WRAPPER`, `BLOCKING_TOKIO`, `REQUIRE_SQL_MARKER`, `STRICT_SQLX`,
/// `SILENT_SATURATION`, `UNBOUNDED_CHANNEL`, `BOOL_PARAMS`) are off unless their
/// variable is set to any value; the SQL-seam, file-length, wildcard-import, and
/// dynamic-SQLx passes are always active (file-length only fires once
/// `MAX_FILE_LINES` is set).
mod config {
    use super::*;

    /// `RUST_LINTS_MAX_FILE_LINES`: max source lines before the file-length pass
    /// fires. Unset disables the pass entirely. No default.
    pub fn max_file_lines() -> Option<usize> {
        env::var("RUST_LINTS_MAX_FILE_LINES")
            .ok()
            .and_then(|value| value.parse().ok())
    }

    /// `RUST_LINTS_HTTP_WRAPPER`: feature gate for the outbound-HTTP-wrapper pass.
    pub fn http_wrapper_enabled() -> bool {
        env::var_os("RUST_LINTS_HTTP_WRAPPER").is_some()
    }

    /// `RUST_LINTS_BLOCKING_TOKIO`: feature gate for the blocking-in-async pass.
    pub fn blocking_tokio_enabled() -> bool {
        env::var_os("RUST_LINTS_BLOCKING_TOKIO").is_some()
    }

    /// `RUST_LINTS_REQUIRE_SQL_MARKER`: feature gate for the inline `--sql` marker pass.
    pub fn require_sql_marker_enabled() -> bool {
        env::var_os("RUST_LINTS_REQUIRE_SQL_MARKER").is_some()
    }

    /// `RUST_LINTS_STRICT_SQLX`: feature gate for the compile-time-SQLx-macro pass.
    pub fn strict_sqlx_enabled() -> bool {
        env::var_os("RUST_LINTS_STRICT_SQLX").is_some()
    }

    /// `RUST_LINTS_SILENT_SATURATION`: feature gate for the silent
    /// numeric-saturation / default-substitution pass.
    pub fn silent_saturation_enabled() -> bool {
        env::var_os("RUST_LINTS_SILENT_SATURATION").is_some()
    }

    /// `RUST_LINTS_UNBOUNDED_CHANNEL`: feature gate for the unbounded-channel pass.
    pub fn unbounded_channel_enabled() -> bool {
        env::var_os("RUST_LINTS_UNBOUNDED_CHANNEL").is_some()
    }

    /// `RUST_LINTS_BOOL_PARAMS`: feature gate for the boolean-parameter-on-a-
    /// public-fn pass.
    pub fn bool_params_enabled() -> bool {
        env::var_os("RUST_LINTS_BOOL_PARAMS").is_some()
    }

    /// `RUST_LINTS_LABEL`: free-text platform label woven into messages
    /// (e.g. "orders platform"). Default: "platform".
    pub fn label() -> String {
        env::var("RUST_LINTS_LABEL").unwrap_or_else(|_| "platform".to_owned())
    }

    /// `RUST_LINTS_SQLX_OWNER_PATHS`: comma-separated path substrings that are
    /// allowed to use SQLx / database adapters directly (the reviewed db seam).
    /// Falls back to `RUST_LINTS_SEAM_CRATES` if unset, then to a built-in default.
    pub fn sqlx_owner_paths() -> Vec<String> {
        list_var("RUST_LINTS_SQLX_OWNER_PATHS")
            .or_else(|| list_var("RUST_LINTS_SEAM_CRATES"))
            .unwrap_or_else(|| {
                vec![
                    "crates/platform-db/".to_owned(),
                    "crates/platform-infra/".to_owned(),
                    "crates/platform-infrastructure/".to_owned(),
                ]
            })
    }

    /// `RUST_LINTS_HTTP_OWNER_PATHS`: comma-separated path substrings allowed to
    /// construct outbound HTTP clients directly (the reviewed http wrapper seam).
    /// Falls back to `RUST_LINTS_SEAM_CRATES` if unset, then to a built-in default.
    pub fn http_owner_paths() -> Vec<String> {
        list_var("RUST_LINTS_HTTP_OWNER_PATHS")
            .or_else(|| list_var("RUST_LINTS_SEAM_CRATES"))
            .unwrap_or_else(|| {
                vec![
                    "crates/platform-http/src/".to_owned(),
                    "crates/platform-observability/src/telemetry.rs".to_owned(),
                    "apps/".to_owned(),
                ]
            })
    }

    /// `RUST_LINTS_REPO_SOURCE_ROOTS`: comma-separated path prefixes the
    /// file-length pass scans. Default: `apps/,crates/` plus the fixtures dir.
    pub fn repo_source_roots() -> Vec<String> {
        list_var("RUST_LINTS_REPO_SOURCE_ROOTS").unwrap_or_else(|| {
            let mut roots = vec!["apps/".to_owned(), "crates/".to_owned()];
            roots.push(fixture_path_prefix());
            roots
        })
    }

    /// `RUST_LINTS_FIXTURE_PATH`: path substring identifying the lint's own
    /// violation fixtures, exempted from the SQL-seam and HTTP-wrapper passes.
    /// Default: `tools/rust-lints/fixtures/`.
    pub fn fixture_path_prefix() -> String {
        env::var("RUST_LINTS_FIXTURE_PATH")
            .unwrap_or_else(|_| "tools/rust-lints/fixtures/".to_owned())
    }

    /// `RUST_LINTS_EXCEPTION_MARKER`: keyword for an inline documented
    /// file-length exception. Default: `rust-lints-file-length-exception`.
    pub fn exception_marker() -> String {
        env::var("RUST_LINTS_EXCEPTION_MARKER")
            .unwrap_or_else(|_| "rust-lints-file-length-exception".to_owned())
    }

    /// `RUST_LINTS_DYNAMIC_SQL_MARKER`: keyword for an inline documented
    /// dynamic-SQL safety note. Default: `rust-lints-dynamic-sql`.
    pub fn dynamic_sql_marker() -> String {
        env::var("RUST_LINTS_DYNAMIC_SQL_MARKER")
            .unwrap_or_else(|_| "rust-lints-dynamic-sql".to_owned())
    }

    /// `RUST_LINTS_FILE_LENGTH_EXCEPTIONS`: path to a TSV inventory of allowed
    /// oversized files (first column = repo-relative path, header row skipped).
    /// Unset disables the inventory lookup (inline markers still apply).
    pub fn file_length_exceptions_path() -> Option<PathBuf> {
        env::var("RUST_LINTS_FILE_LENGTH_EXCEPTIONS")
            .ok()
            .map(PathBuf::from)
    }

    fn list_var(name: &str) -> Option<Vec<String>> {
        env::var(name).ok().map(|value| {
            value
                .split(',')
                .map(|entry| entry.trim().to_owned())
                .filter(|entry| !entry.is_empty())
                .collect()
        })
    }
}

impl<'tcx> LateLintPass<'tcx> for RustLintsPolicyChecks {
    fn check_crate(&mut self, cx: &LateContext<'tcx>) {
        check_file_lengths(cx);
    }

    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        let snippet = snippet(cx, item.span);

        check_wildcard_import(cx, item);
        check_sqlx_boundary(cx, item.span, &snippet);
    }

    fn check_ty(&mut self, cx: &LateContext<'tcx>, ty: &'tcx Ty<'tcx, AmbigArg>) {
        if matches!(ty.kind, TyKind::Path(_)) {
            let snippet = snippet(cx, ty.span);
            check_sqlx_boundary(cx, ty.span, &snippet);
        }
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let expr_snippet = snippet(cx, expr.span);
        check_sqlx_boundary(cx, expr.span, &expr_snippet);
        check_outbound_http_wrapper(cx, expr.span, &expr_snippet);
        check_inline_sql_marker(cx, expr.span, &expr_snippet, &expr.kind);
        check_blocking_call_in_async_context(cx, expr, &expr_snippet);
        check_silent_numeric_saturation(cx, expr);
        check_unbounded_channel(cx, expr);

        if let ExprKind::Call(callee, _) = expr.kind {
            let callee_snippet = snippet(cx, callee.span);
            if is_sqlx_runtime_query(&callee_snippet) {
                check_dynamic_sqlx_safety(cx, expr.span, &expr_snippet);
                check_strict_compile_time_sqlx(cx, expr.span, &expr_snippet);
            }
        }
    }

    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        kind: FnKind<'tcx>,
        decl: &'tcx FnDecl<'tcx>,
        _body: &'tcx Body<'tcx>,
        span: Span,
        def_id: LocalDefId,
    ) {
        check_bool_param_on_public_fn(cx, kind, decl, span, def_id);
    }
}

fn check_outbound_http_wrapper(cx: &LateContext<'_>, span: Span, snippet: &str) {
    if !config::http_wrapper_enabled()
        || in_http_owner_path(cx, span)
        || in_lint_fixture_path(cx, span)
    {
        return;
    }

    if contains_direct_reqwest_client(snippet) {
        emit(
            cx,
            span,
            format!(
                "outbound HTTP clients must go through reviewed {} wrappers",
                config::label()
            ),
            "move runtime HTTP calls behind wrapper paths so URL policy, redirects, PII/PHI-safe logging, retry behavior, and evidence redaction stay enforced".to_owned(),
        );
    }
}

fn check_blocking_call_in_async_context(cx: &LateContext<'_>, expr: &Expr<'_>, expr_snippet: &str) {
    if !config::blocking_tokio_enabled() {
        return;
    }

    if !in_async_context_without_blocking_quarantine(cx, expr) {
        return;
    }

    if is_blocking_call(cx, expr, expr_snippet) {
        emit(
            cx,
            expr.span,
            "blocking calls must not run on Tokio async worker threads".to_owned(),
            "move blocking work behind tokio::task::spawn_blocking or replace it with Tokio async APIs".to_owned(),
        );
    }
}

fn check_file_lengths(cx: &LateContext<'_>) {
    let Some(max_lines) = config::max_file_lines() else {
        return;
    };

    for source_file in cx.sess().source_map().files().iter() {
        let Some(path) = source_file_local_path(source_file) else {
            continue;
        };
        if !is_repo_rust_source_path(&path) {
            continue;
        }

        let key = normalize_path(&path);
        if !mark_file_seen(&key) {
            continue;
        }

        let Some(contents) = source_file_contents(source_file, &path) else {
            continue;
        };

        let line_count = contents.lines().count();
        if line_count <= max_lines || has_documented_file_length_exception(&key, &contents) {
            continue;
        }

        emit(
            cx,
            DUMMY_SP,
            "Rust source files must stay small enough to parse and review".to_owned(),
            format!(
                "split large files into focused modules using directory modules and mod.rs, or add a documented {} with owner, reason, and expires fields",
                config::exception_marker()
            ),
        );
    }
}

fn check_wildcard_import(cx: &LateContext<'_>, item: &Item<'_>) {
    if !matches!(item.kind, ItemKind::Use(_, UseKind::Glob)) {
        return;
    }

    emit(
        cx,
        item.span,
        "wildcard imports are not allowed".to_owned(),
        "replace wildcard imports with explicitly named imports or re-exports".to_owned(),
    );
}

fn source_file_local_path(source_file: &rustc_span::SourceFile) -> Option<PathBuf> {
    match &source_file.name {
        FileName::Real(real) => real.clone().into_local_path(),
        _ => None,
    }
}

fn source_file_contents(source_file: &rustc_span::SourceFile, path: &Path) -> Option<String> {
    if let Some(contents) = source_file.src.as_ref() {
        return Some(contents.to_string());
    }
    fs::read_to_string(path).ok()
}

fn mark_file_seen(key: &str) -> bool {
    static SEEN_FILES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let seen_files = SEEN_FILES.get_or_init(|| Mutex::new(BTreeSet::new()));
    let Ok(mut seen_files) = seen_files.lock() else {
        return false;
    };
    seen_files.insert(key.to_owned())
}

fn is_repo_rust_source_path(path: &Path) -> bool {
    is_repo_rust_source_path_with_roots(path, &config::repo_source_roots())
}

fn is_repo_rust_source_path_with_roots(path: &Path, roots: &[String]) -> bool {
    if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
        return false;
    }

    let normalized_path = normalized_path_buf(path);
    let normalized = env::current_dir()
        .ok()
        .map(|current_dir| normalized_path_buf(&current_dir))
        .and_then(|current_dir| {
            normalized_path
                .strip_prefix(current_dir)
                .ok()
                .map(normalize_path)
        })
        .unwrap_or_else(|| normalize_path(&normalized_path));

    roots.iter().any(|root| normalized.starts_with(root))
}

fn has_documented_file_length_exception(path: &str, contents: &str) -> bool {
    has_inventory_file_length_exception(path) || has_inline_file_length_exception(contents)
}

fn has_inventory_file_length_exception(path: &str) -> bool {
    let Some(inventory_path) = config::file_length_exceptions_path() else {
        return false;
    };
    let Ok(inventory) = fs::read_to_string(inventory_path) else {
        return false;
    };
    inventory_lists_path(&inventory, path)
}

fn inventory_lists_path(inventory: &str, path: &str) -> bool {
    inventory
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once('\t'))
        .any(|(exception_path, _)| exception_path == path)
}

fn has_inline_file_length_exception(contents: &str) -> bool {
    has_inline_file_length_exception_with_marker(contents, &config::exception_marker())
}

fn has_inline_file_length_exception_with_marker(contents: &str, marker: &str) -> bool {
    let needle = format!("{marker}:");
    contents.lines().take(80).any(|line| {
        line.contains(&needle)
            && line.contains("owner=")
            && line.contains("reason=")
            && line.contains("expires=")
    })
}

fn check_inline_sql_marker(
    cx: &LateContext<'_>,
    span: Span,
    snippet: &str,
    expr_kind: &ExprKind<'_>,
) {
    if !config::require_sql_marker_enabled() {
        return;
    }

    if in_non_production_target(cx, span) {
        return;
    }

    if !matches!(expr_kind, ExprKind::Lit(_)) {
        return;
    }

    if contains_inline_sql_text(snippet) && !string_literal_starts_with_sql_marker(snippet) {
        emit(
            cx,
            span,
            "inline SQL string literals must start with --sql".to_owned(),
            "prefix SQL literals with --sql so editor SQL highlighting works consistently"
                .to_owned(),
        );
    }
}

fn check_sqlx_boundary(cx: &LateContext<'_>, span: Span, snippet: &str) {
    if in_sqlx_owner_path(cx, span)
        || in_lint_fixture_path(cx, span)
        || in_non_production_target(cx, span)
    {
        return;
    }

    if contains_sqlx_boundary_token(snippet) {
        emit(
            cx,
            span,
            "SQLx and database adapter APIs must stay in reviewed database infrastructure paths"
                .to_owned(),
            "move SQL and pool usage behind a reviewed database, storage, audit, or infrastructure crate boundary".to_owned(),
        );
    }
}

fn check_dynamic_sqlx_safety(cx: &LateContext<'_>, span: Span, snippet: &str) {
    if !in_sqlx_owner_path(cx, span) || in_non_production_target(cx, span) {
        return;
    }

    if contains_static_sql_literal(snippet) {
        return;
    }

    if contains_dynamic_sql_argument(snippet)
        && !snippet.contains("AssertSqlSafe")
        && !has_dynamic_sql_note(cx, span)
    {
        emit(
            cx,
            span,
            "dynamic SQLx query constructors must cross a documented SQL safety boundary"
                .to_owned(),
            "validate identifiers and SQL shape before calling sqlx::query with dynamic SQL"
                .to_owned(),
        );
    }
}

fn check_strict_compile_time_sqlx(cx: &LateContext<'_>, span: Span, snippet: &str) {
    if !config::strict_sqlx_enabled() {
        return;
    }

    if in_non_production_target(cx, span) {
        return;
    }

    if snippet.contains("AssertSqlSafe") {
        return;
    }

    if contains_static_sql_literal(snippet) {
        emit(
            cx,
            span,
            "static SQLx queries should use compile-time checked macros".to_owned(),
            "prefer sqlx::query!, query_as!, or query_scalar! once SQLx offline metadata covers this query".to_owned(),
        );
    }
}

/// Pass: silent numeric saturation / default substitution.
///
/// Flags a fallible numeric conversion whose error is silently swallowed by one
/// of the `unwrap_or` family, e.g. `i32::try_from(count).unwrap_or(i32::MAX)`,
/// `x.try_into().unwrap_or_default()`, or `u8::try_from(n).unwrap_or_else(..)`.
/// These compile cleanly and clippy never flags them, yet they quietly corrupt a
/// persisted/wire value when the input is out of range.
///
/// Match strategy (HIR + types, conservative):
/// 1. The outer expression is a `MethodCall` whose method is one of the
///    `unwrap_or` family.
/// 2. Its receiver is itself a `MethodCall`/`Call` to `try_from`/`try_into`.
/// 3. The receiver's type is `Result<T, _>` whose `Ok` type `T` is a primitive
///    integer or float. Keying on the *resolved* `Ok` type (not the syntactic
///    target) keeps us off non-numeric `TryFrom` impls.
fn check_silent_numeric_saturation(cx: &LateContext<'_>, expr: &Expr<'_>) {
    if !config::silent_saturation_enabled() {
        return;
    }

    let ExprKind::MethodCall(segment, receiver, _, _) = expr.kind else {
        return;
    };
    if !is_unwrap_or_family(segment.ident.as_str()) {
        return;
    }
    if !receiver_is_fallible_conversion(receiver) {
        return;
    }
    if !conversion_result_ok_type_is_numeric(cx, receiver) {
        return;
    }

    emit(
        cx,
        expr.span,
        "numeric conversion silently saturates/substitutes a default on overflow — reject or handle the out-of-range value explicitly".to_owned(),
        "match on the TryFrom/TryInto result and surface the out-of-range case instead of falling back to a saturated or default value".to_owned(),
    );
}

/// The `unwrap_or` family that silently discards the conversion error.
fn is_unwrap_or_family(method_name: &str) -> bool {
    matches!(
        method_name,
        "unwrap_or" | "unwrap_or_default" | "unwrap_or_else"
    )
}

/// True when `receiver` is syntactically a `try_from(..)` / `try_into(..)` call.
fn receiver_is_fallible_conversion(receiver: &Expr<'_>) -> bool {
    match receiver.kind {
        // `x.try_into()` (and the rare `x.try_from(..)` method form).
        ExprKind::MethodCall(segment, _, _, _) => {
            is_fallible_conversion_name(segment.ident.as_str())
        }
        // `T::try_from(x)` / `<T as TryFrom<_>>::try_from(x)`.
        ExprKind::Call(callee, _) => {
            last_path_segment(callee).is_some_and(|name| is_fallible_conversion_name(name))
        }
        _ => false,
    }
}

fn is_fallible_conversion_name(name: &str) -> bool {
    matches!(name, "try_from" | "try_into")
}

/// The trailing path segment of a callee expression, e.g. `try_from` in
/// `i32::try_from`.
fn last_path_segment<'a>(callee: &'a Expr<'a>) -> Option<&'a str> {
    let ExprKind::Path(qpath) = &callee.kind else {
        return None;
    };
    match qpath {
        rustc_hir::QPath::Resolved(_, path) => {
            path.segments.last().map(|segment| segment.ident.as_str())
        }
        rustc_hir::QPath::TypeRelative(_, segment) => Some(segment.ident.as_str()),
    }
}

/// True when the conversion expression has type `Result<T, _>` and `T` is a
/// primitive integer or float. This is the conservative gate that keeps the pass
/// off non-numeric `TryFrom` impls (e.g. `Foo::try_from(bytes)`).
fn conversion_result_ok_type_is_numeric(cx: &LateContext<'_>, receiver: &Expr<'_>) -> bool {
    let result_ty = cx.typeck_results().expr_ty(receiver);
    let ty::Adt(adt_def, args) = result_ty.kind() else {
        return false;
    };
    if !cx
        .tcx
        .is_diagnostic_item(rustc_span::sym::Result, adt_def.did())
    {
        return false;
    }
    let Some(ok_ty) = args.types().next() else {
        return false;
    };
    ty_is_numeric_primitive(ok_ty)
}

fn ty_is_numeric_primitive(ty: ty::Ty<'_>) -> bool {
    matches!(ty.kind(), ty::Int(_) | ty::Uint(_) | ty::Float(_))
}

/// Pass: unbounded channel construction.
///
/// Flags calls to `tokio::sync::mpsc::unbounded_channel`,
/// `futures::channel::mpsc::unbounded`, and `crossbeam_channel::unbounded` by
/// fully-qualified def path — an unbounded channel has no backpressure bound.
fn check_unbounded_channel(cx: &LateContext<'_>, expr: &Expr<'_>) {
    if !config::unbounded_channel_enabled() {
        return;
    }

    let ExprKind::Call(callee, _) = expr.kind else {
        return;
    };
    let Some(path) = callee_def_path(cx, callee) else {
        return;
    };
    if !is_unbounded_channel_path(&path) {
        return;
    }

    emit(
        cx,
        expr.span,
        "unbounded channel has no backpressure bound — use a bounded channel sized to the workload".to_owned(),
        "replace the unbounded constructor with a bounded channel (e.g. tokio::sync::mpsc::channel(cap)) sized to the workload".to_owned(),
    );
}

/// Match the full module path of the constructor. We compare on the *module-
/// qualified suffix* (not bare `==`) so a re-export shim or a fixture stand-in
/// module — which `def_path_str` renders crate-prefixed, e.g.
/// `mycrate::tokio::sync::mpsc::unbounded_channel` — still matches, while a
/// same-named-but-unrelated free function (`mycrate::unbounded`) does not.
fn is_unbounded_channel_path(path: &str) -> bool {
    [
        "tokio::sync::mpsc::unbounded_channel",
        "futures::channel::mpsc::unbounded",
        "futures_channel::mpsc::unbounded",
        "crossbeam_channel::unbounded",
        "crossbeam::channel::unbounded",
    ]
    .iter()
    .any(|target| path == *target || path.ends_with(&format!("::{target}")))
}

/// Pass: boolean parameter on a public function.
///
/// Flags `pub` / `pub(crate)` free functions and inherent/trait methods whose
/// declaration takes a `bool` parameter — `f(true, false)` is blind at the call
/// site. Test items and obvious `set_*` / `with_*` setters are skipped; the env
/// gate keeps the rest opt-in.
fn check_bool_param_on_public_fn(
    cx: &LateContext<'_>,
    kind: FnKind<'_>,
    decl: &FnDecl<'_>,
    span: Span,
    def_id: LocalDefId,
) {
    if !config::bool_params_enabled() {
        return;
    }

    // A `cargo test` / `--test` build recompiles the crate with `#[cfg(test)]`
    // active and a synthesized harness; that build is not the public API surface
    // we are auditing, and `#[cfg(test)]` attributes have already been stripped
    // by cfg-expansion, so skip the whole compilation.
    if cx.tcx.sess.is_test_crate() {
        return;
    }

    // Closures have no visibility / name to reason about.
    if matches!(kind, FnKind::Closure) {
        return;
    }

    if let Some(name) = fn_kind_name(kind)
        && is_exempt_fn_name(&name)
    {
        return;
    }

    if is_in_test_context(cx, def_id) {
        return;
    }

    if !fn_is_publicly_reachable(cx, def_id) {
        return;
    }

    if !decl_has_bool_input(cx, decl) {
        return;
    }

    emit(
        cx,
        span,
        "boolean parameter is blind at call sites — use a named two-variant enum".to_owned(),
        "replace the bool parameter with a named two-variant enum so call sites read self-documentingly".to_owned(),
    );
}

fn fn_kind_name(kind: FnKind<'_>) -> Option<String> {
    match kind {
        FnKind::ItemFn(ident, _, _) => Some(ident.as_str().to_owned()),
        FnKind::Method(ident, _) => Some(ident.as_str().to_owned()),
        FnKind::Closure => None,
    }
}

/// Cheap setter-style exemptions: a `bool` flag on `set_*` / `with_*` reads fine
/// at the call site already.
fn is_exempt_fn_name(name: &str) -> bool {
    name.starts_with("set_") || name.starts_with("with_")
}

/// True when any declared input is written as the primitive `bool` type.
///
/// We key on the syntactic `bool` path rather than resolving the parameter type
/// so the matcher stays robust without body typeck: a bare, single-segment
/// `bool` path is unambiguous, and a shadowing local `type Bool = ...` aliased to
/// the name `bool` is not something Rust permits.
fn decl_has_bool_input(_cx: &LateContext<'_>, decl: &FnDecl<'_>) -> bool {
    decl.inputs.iter().any(hir_ty_is_bool)
}

fn hir_ty_is_bool(hir_ty: &Ty<'_>) -> bool {
    let TyKind::Path(rustc_hir::QPath::Resolved(_, path)) = &hir_ty.kind else {
        return false;
    };
    path.segments.len() == 1
        && path
            .segments
            .last()
            .is_some_and(|segment| segment.ident.as_str() == "bool")
}

/// Public reachability: we flag functions that are reachable as part of the
/// crate's API surface — i.e. `pub` and `pub(crate)` items in a public path.
///
/// We use the crate's *effective* visibilities rather than the raw syntactic
/// `tcx.visibility`, because the latter cannot tell a crate-root private `fn`
/// (`Restricted(crate-root)`) apart from a `pub(crate)` one. A private fn, or a
/// `pub` fn buried in a private module, has effective visibility below the
/// reachable level and is correctly skipped.
fn fn_is_publicly_reachable(cx: &LateContext<'_>, def_id: LocalDefId) -> bool {
    cx.tcx.effective_visibilities(()).is_reachable(def_id)
}

fn is_in_test_context(cx: &LateContext<'_>, def_id: LocalDefId) -> bool {
    let hir_id = cx.tcx.local_def_id_to_hir_id(def_id);
    for (_, node) in
        std::iter::once((hir_id, cx.tcx.hir_node(hir_id))).chain(cx.tcx.hir_parent_iter(hir_id))
    {
        let attrs = match node {
            Node::Item(item) => cx.tcx.hir_attrs(item.hir_id()),
            Node::ImplItem(item) => cx.tcx.hir_attrs(item.hir_id()),
            Node::TraitItem(item) => cx.tcx.hir_attrs(item.hir_id()),
            _ => continue,
        };
        if attrs_mark_test(attrs) {
            return true;
        }
    }
    false
}

fn attrs_mark_test(attrs: &[rustc_hir::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.has_name(rustc_span::sym::test)
            || (attr.has_name(rustc_span::sym::cfg)
                && attr
                    .meta_item_list()
                    .into_iter()
                    .flatten()
                    .any(|nested| nested.has_name(rustc_span::sym::test)))
    })
}

fn in_sqlx_owner_path(cx: &LateContext<'_>, span: Span) -> bool {
    config::sqlx_owner_paths()
        .iter()
        .any(|needle| in_path(cx, span, needle))
}

fn in_http_owner_path(cx: &LateContext<'_>, span: Span) -> bool {
    config::http_owner_paths()
        .iter()
        .any(|needle| in_path(cx, span, needle))
}

fn in_lint_fixture_path(cx: &LateContext<'_>, span: Span) -> bool {
    in_path(cx, span, &config::fixture_path_prefix())
}

/// Non-production compilation targets that the SQLx-policy passes do not audit:
/// the `--test` build (the synthesized harness plus `#[cfg(test)]` code) and
/// `examples/` crates (lab-gated live-DB evidence scripts).
///
/// The SQLx-policy lints (`strict_sqlx`, `sqlx_boundary`, `require_sql_marker`,
/// `dynamic_sqlx_safety`) exist to keep PRODUCTION SQL compile-checked, in a
/// reviewed crate, and marked — a runtime `sqlx::query` in a test or an
/// evidence script is expected and is exercised against a real database, so a
/// SQL error there fails the run immediately. Production code stays fully
/// covered: it is linted by the normal `lib`/`bin` build (`is_test_crate()` is
/// false and its file is not under `examples/`). Skipping the test build here
/// loses no production coverage because that same production code is linted in
/// its non-test build.
fn in_non_production_target(cx: &LateContext<'_>, span: Span) -> bool {
    cx.tcx.sess.is_test_crate() || in_path(cx, span, "/examples/")
}

fn in_async_context_without_blocking_quarantine(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    for (_, node) in cx.tcx.hir_parent_iter(expr.hir_id) {
        let Node::Expr(parent) = node else {
            continue;
        };
        let ExprKind::Closure(closure) = parent.kind else {
            continue;
        };
        if closure_is_blocking_quarantine(cx, parent) {
            return false;
        }
        if closure_is_async(closure) {
            return true;
        }
    }
    false
}

fn closure_is_async(closure: &Closure<'_>) -> bool {
    matches!(
        closure.kind,
        ClosureKind::Coroutine(CoroutineKind::Desugared(
            CoroutineDesugaring::Async | CoroutineDesugaring::AsyncGen,
            _
        )) | ClosureKind::CoroutineClosure(
            CoroutineDesugaring::Async | CoroutineDesugaring::AsyncGen
        )
    )
}

fn closure_is_blocking_quarantine(cx: &LateContext<'_>, closure_expr: &Expr<'_>) -> bool {
    match cx.tcx.parent_hir_node(closure_expr.hir_id) {
        Node::Expr(parent) => match parent.kind {
            ExprKind::Call(callee, _) => {
                let callee = snippet(cx, callee.span);
                is_blocking_quarantine_call(&callee)
            }
            ExprKind::MethodCall(segment, _, _, _) => {
                is_blocking_quarantine_call(segment.ident.as_str())
            }
            _ => false,
        },
        _ => false,
    }
}

fn is_blocking_quarantine_call(snippet: &str) -> bool {
    snippet.contains("spawn_blocking")
        || snippet.contains("block_in_place")
        || snippet.contains("std::thread::spawn")
        || snippet == "thread::spawn"
}

fn is_blocking_call(cx: &LateContext<'_>, expr: &Expr<'_>, expr_snippet: &str) -> bool {
    match expr.kind {
        ExprKind::Call(callee, _) => {
            let callee_snippet = snippet(cx, callee.span);
            let path = callee_def_path(cx, callee);
            is_blocking_function_call(&callee_snippet)
                || path.is_some_and(|path| is_blocking_path(&path))
        }
        ExprKind::MethodCall(segment, _, _, _) => {
            let method_name = segment.ident.as_str();
            let path = cx
                .typeck_results()
                .type_dependent_def_id(expr.hir_id)
                .map(|def_id| cx.tcx.def_path_str(def_id));
            is_blocking_method_name(method_name)
                || path.is_some_and(|path| is_blocking_path(&path))
                || expr_snippet.contains("reqwest::blocking")
        }
        _ => false,
    }
}

fn callee_def_path(cx: &LateContext<'_>, callee: &Expr<'_>) -> Option<String> {
    let ExprKind::Path(qpath) = callee.kind else {
        return None;
    };
    cx.qpath_res(&qpath, callee.hir_id)
        .opt_def_id()
        .map(|def_id| cx.tcx.def_path_str(def_id))
}

fn is_blocking_function_call(snippet: &str) -> bool {
    let normalized = snippet.trim();
    is_blocking_path(normalized)
        || normalized.contains("reqwest::blocking")
        || normalized == "block_on"
        || normalized.ends_with("::block_on")
}

fn is_blocking_path(path: &str) -> bool {
    path.starts_with("std::thread::sleep")
        || path.starts_with("std::fs::")
        || path.starts_with("std::net::")
        || path.starts_with("std::process::Command")
        || path.starts_with("std::process::Child")
        || path.starts_with("std::sync::mpsc::")
        || path.starts_with("std::sync::mpmc::")
        || path.contains("reqwest::blocking")
        || path.ends_with("::block_on")
}

fn is_blocking_method_name(method_name: &str) -> bool {
    matches!(
        method_name,
        "block_on" | "recv" | "recv_timeout" | "wait" | "wait_with_output" | "output"
    )
}

fn contains_sqlx_boundary_token(snippet: &str) -> bool {
    // Match boundary tokens only where they are *code* — never where they merely
    // appear inside a string/char literal or a comment (e.g. a PHI-free audit
    // note that says "no PgPool / durable write"). Blanking literal/comment
    // contents keeps a genuine `sqlx::`/`PgPool` path or type flagged while a
    // documentation mention of the same word is not.
    let code = code_without_literals_and_comments(snippet);
    code.contains("sqlx::")
        || code.contains("PgPool")
        || code.contains("PoolOptions")
        || code.contains("sqlx::Postgres")
        || code.contains("AssertSqlSafe")
}

/// Return `snippet` with the *contents* of string, byte-string, and char
/// literals and of line/block comments replaced by spaces, leaving all
/// delimiters and every token outside a literal or comment in place.
///
/// Token checks run over source snippets, so a bare substring match would fire
/// on a word that only appears inside a string literal or a comment. Blanking
/// those regions first makes the checks see real code only. The scanner handles
/// regular/byte strings (with `\` escapes), raw strings (`r"…"`, `r#"…"#`, and
/// the byte-raw forms — no escapes, hash-balanced terminator), char literals
/// (distinguished from lifetimes/labels: a char literal is `'`, one char or an
/// escape, then `'`), and line (`//`) and nested block (`/* … */`) comments.
fn code_without_literals_and_comments(snippet: &str) -> String {
    let chars: Vec<char> = snippet.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(snippet.len());
    let mut i = 0;

    while i < n {
        let c = chars[i];

        // Line comment: `//` to end of line.
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            out.push_str("//");
            i += 2;
            while i < n && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }

        // Block comment: `/* … */`, nested.
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            out.push_str("  ");
            i += 2;
            let mut depth = 1usize;
            while i < n && depth > 0 {
                if chars[i] == '/' && i + 1 < n && chars[i + 1] == '*' {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && i + 1 < n && chars[i + 1] == '/' {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                } else {
                    out.push(' ');
                    i += 1;
                }
            }
            continue;
        }

        // Raw string: optional `b`, then `r`, then N `#`, then `"`; body ends at
        // `"` followed by the same N `#`. No escape processing.
        if c == 'r' || (c == 'b' && i + 1 < n && chars[i + 1] == 'r') {
            let start = i;
            let mut j = i;
            if chars[j] == 'b' {
                j += 1;
            }
            // chars[j] == 'r'
            let mut k = j + 1;
            let mut hashes = 0usize;
            while k < n && chars[k] == '#' {
                hashes += 1;
                k += 1;
            }
            if k < n && chars[k] == '"' {
                for &pc in &chars[start..=k] {
                    out.push(pc);
                }
                i = k + 1;
                loop {
                    if i >= n {
                        break;
                    }
                    if chars[i] == '"' {
                        let mut m = i + 1;
                        let mut cnt = 0;
                        while m < n && cnt < hashes && chars[m] == '#' {
                            cnt += 1;
                            m += 1;
                        }
                        if cnt == hashes {
                            out.push('"');
                            for _ in 0..hashes {
                                out.push('#');
                            }
                            i = i + 1 + hashes;
                            break;
                        }
                    }
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
            // Not a raw string (e.g. an identifier like `row`); fall through.
        }

        // Regular or byte string: `"…"` / `b"…"` with `\` escapes.
        if c == '"' || (c == 'b' && i + 1 < n && chars[i + 1] == '"') {
            if c == 'b' {
                out.push('b');
                i += 1;
            }
            out.push('"');
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    out.push('"');
                    i += 1;
                    break;
                }
                out.push(' ');
                i += 1;
            }
            continue;
        }

        // Char literal vs lifetime/label. A char literal is `'` then one char or
        // an escape then `'`; anything else beginning with `'` is a lifetime or
        // loop label and is left as ordinary code.
        if c == '\'' {
            if i + 1 < n && chars[i + 1] == '\\' {
                // Escaped char literal: `'` `\` <escaped> [payload…] `'`.
                out.push('\'');
                i += 1; // at backslash
                out.push(' ');
                i += 1; // blank backslash
                if i < n {
                    out.push(' ');
                    i += 1; // blank the escaped char
                }
                while i < n && chars[i] != '\'' {
                    out.push(' ');
                    i += 1;
                }
                if i < n {
                    out.push('\'');
                    i += 1;
                }
                continue;
            }
            if i + 2 < n && chars[i + 2] == '\'' {
                // Simple char literal: `'X'`.
                out.push('\'');
                out.push(' ');
                out.push('\'');
                i += 3;
                continue;
            }
            // Lifetime / label tick.
            out.push('\'');
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }

    out
}

fn contains_dynamic_sql_argument(snippet: &str) -> bool {
    snippet.contains("format!(")
        || snippet.contains(".as_str()")
        || snippet.contains("query(sql)")
        || snippet.contains("query(&sql)")
        || snippet.contains("query(statement)")
        || snippet.contains("query(&statement)")
        || snippet.contains("query(query)")
        || snippet.contains("query(&query)")
}

fn contains_static_sql_literal(snippet: &str) -> bool {
    let normalized = snippet.to_ascii_lowercase();
    (normalized.contains("query(\"")
        || normalized.contains("query(\n")
        || normalized.contains("query_scalar(\n")
        || normalized.contains("query_scalar::<")
        || normalized.contains("query_as(\n")
        || normalized.contains("query_as::<"))
        && (normalized.contains("select ")
            || normalized.contains("insert into")
            || normalized.contains("update ")
            || normalized.contains("delete from"))
}

fn contains_inline_sql_text(snippet: &str) -> bool {
    let normalized = snippet.to_ascii_lowercase();
    (normalized.contains("select ") && normalized.contains(" from"))
        || normalized.contains("insert into")
        || normalized.contains("update ") && normalized.contains(" set")
        || normalized.contains("delete from")
        || normalized.contains("create schema")
        || normalized.contains("create materialized view")
        || normalized.contains("create or replace view")
        || normalized.contains("drop materialized view")
        || normalized.contains("drop view")
        || normalized.contains("alter materialized view")
        || normalized.contains("grant select")
}

fn string_literal_starts_with_sql_marker(snippet: &str) -> bool {
    string_literal_body(snippet).is_some_and(|body| body.trim_start().starts_with("--sql"))
}

fn string_literal_body(snippet: &str) -> Option<&str> {
    let trimmed = snippet.trim_start();

    if let Some(raw) = trimmed.strip_prefix('r') {
        let quote_index = raw.find('"')?;
        return Some(&raw[quote_index + 1..]);
    }

    if let Some(byte_string) = trimmed.strip_prefix('b') {
        return byte_string.strip_prefix('"');
    }

    trimmed.strip_prefix('"')
}

fn is_sqlx_runtime_query(snippet: &str) -> bool {
    [
        "sqlx::query",
        "sqlx::query_as",
        "sqlx::query_scalar",
        "sqlx::query_file",
        "sqlx::query_file_as",
        "sqlx::query_file_scalar",
    ]
    .iter()
    .any(|name| snippet == *name || snippet.contains(&format!("{name}(")))
}

fn contains_direct_reqwest_client(snippet: &str) -> bool {
    [
        "reqwest::Client::new",
        "reqwest::Client::builder",
        "reqwest::blocking::Client::new",
        "reqwest::blocking::Client::builder",
        "reqwest::get",
        "reqwest::blocking::get",
    ]
    .iter()
    .any(|token| snippet.contains(token))
}

fn has_dynamic_sql_note(cx: &LateContext<'_>, span: Span) -> bool {
    let Some(path) = local_path(cx, span) else {
        return false;
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };

    let marker = format!("{}: ", config::dynamic_sql_marker());
    let line = cx.sess().source_map().lookup_char_pos(span.lo()).line;
    let start = line.saturating_sub(5);
    contents
        .lines()
        .skip(start)
        .take(line.saturating_sub(start))
        .any(|line| line.contains(&marker))
}

fn in_path(cx: &LateContext<'_>, span: Span, needle: &str) -> bool {
    local_path(cx, span).is_some_and(|path| normalize_path(&path).contains(needle))
}

fn local_path(cx: &LateContext<'_>, span: Span) -> Option<PathBuf> {
    match cx.sess().source_map().span_to_filename(span) {
        FileName::Real(real) => real.into_local_path(),
        _ => None,
    }
}

fn normalize_path(path: &Path) -> String {
    normalized_path_buf(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalized_path_buf(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn snippet(cx: &LateContext<'_>, span: Span) -> String {
    cx.sess()
        .source_map()
        .span_to_snippet(span)
        .unwrap_or_default()
}

fn emit(cx: &LateContext<'_>, span: Span, message: String, help: String) {
    cx.emit_span_lint(
        RUST_LINTS_POLICY_CHECKS,
        MultiSpan::from_span(span),
        RustLintsPolicyDiag { message, help },
    );
}

struct RustLintsPolicyDiag {
    message: String,
    help: String,
}

impl<'a, G: EmissionGuarantee> Diagnostic<'a, G> for RustLintsPolicyDiag {
    fn into_diag(self, dcx: rustc_errors::DiagCtxtHandle<'a>, level: Level) -> Diag<'a, G> {
        let mut diag = Diag::new(dcx, level, self.message);
        diag.help(self.help);
        diag
    }
}

#[cfg(test)]
mod tests {
    use super::{
        config, contains_direct_reqwest_client, contains_dynamic_sql_argument,
        contains_inline_sql_text, contains_sqlx_boundary_token, contains_static_sql_literal,
        has_inline_file_length_exception_with_marker, inventory_lists_path,
        is_blocking_function_call, is_blocking_method_name, is_blocking_path,
        is_blocking_quarantine_call, is_exempt_fn_name, is_fallible_conversion_name,
        is_repo_rust_source_path_with_roots, is_sqlx_runtime_query, is_unbounded_channel_path,
        is_unwrap_or_family, string_literal_starts_with_sql_marker,
    };

    #[test]
    fn recognizes_sqlx_boundary_tokens() {
        assert!(contains_sqlx_boundary_token("sqlx::query(\"select 1\")"));
        assert!(contains_sqlx_boundary_token("let pool: PgPool = pool;"));
        assert!(contains_sqlx_boundary_token("PoolOptions::new()"));
        assert!(contains_sqlx_boundary_token("AssertSqlSafe(sql)"));
        assert!(!contains_sqlx_boundary_token("let value = 42;"));
    }

    #[test]
    fn ignores_boundary_tokens_inside_literals_and_comments() {
        // A boundary word that appears only inside a string literal is a
        // documentation mention, not database-adapter usage.
        assert!(!contains_sqlx_boundary_token(
            "\"read-only exit-package checksum verification; no PgPool / durable write\""
        ));
        // The same holds when the literal is one argument of an enclosing call
        // (the whole-expression snippet still contains the string).
        assert!(!contains_sqlx_boundary_token(
            "row(command, exempt(E::NoDurableSideEffect), \"no PgPool / durable write\")"
        ));
        // Raw strings and comments are covered too.
        assert!(!contains_sqlx_boundary_token(
            "let note = r#\"emitted alongside sqlx::query in the writer\"#;"
        ));
        assert!(!contains_sqlx_boundary_token("// this row reaches no PgPool"));
        assert!(!contains_sqlx_boundary_token(
            "let n = 1; /* PgPool lives in platform-db */ let m = 2;"
        ));
        // Real code usage is still flagged even with an adjacent note string.
        assert!(contains_sqlx_boundary_token(
            "let pool: PgPool = get(); // returns the PgPool"
        ));
        assert!(contains_sqlx_boundary_token(
            "sqlx::query(\"select 1\").execute(&pool)"
        ));
        // A lifetime/label beginning with `'` must not swallow following code.
        assert!(contains_sqlx_boundary_token(
            "fn f<'a>(p: &'a PgPool) -> &'a PgPool { p }"
        ));
    }

    #[test]
    fn recognizes_runtime_sqlx_query_constructors() {
        assert!(is_sqlx_runtime_query("sqlx::query(\"select 1\")"));
        assert!(is_sqlx_runtime_query(
            "sqlx::query_file_scalar(\"query.sql\")"
        ));
        assert!(!is_sqlx_runtime_query("sqlx::query!(\"select 1\")"));
    }

    #[test]
    fn recognizes_dynamic_sql_arguments() {
        assert!(contains_dynamic_sql_argument(
            "sqlx::query(format!(\"select * from {table}\"))"
        ));
        assert!(contains_dynamic_sql_argument("sqlx::query(query.as_str())"));
        assert!(!contains_dynamic_sql_argument(
            "sqlx::query(\"select id from workers\")"
        ));
    }

    #[test]
    fn recognizes_static_sql_literals() {
        assert!(contains_static_sql_literal(
            "sqlx::query(\"select id from workers\")"
        ));
        assert!(contains_static_sql_literal(
            "sqlx::query_scalar::<_, bool>(\"SELECT EXISTS (SELECT 1 FROM elections)\")"
        ));
        assert!(!contains_static_sql_literal(
            "sqlx::query(AssertSqlSafe(format!(\"select * from {table}\")))"
        ));
    }

    #[test]
    fn recognizes_inline_sql_text() {
        assert!(contains_inline_sql_text(
            r#""SELECT election_id, status FROM elections""#
        ));
        assert!(contains_inline_sql_text(
            r#""CREATE OR REPLACE VIEW enrollment.open_windows AS SELECT 1""#
        ));
        assert!(!contains_inline_sql_text(
            r#""definition must start with SELECT or WITH""#
        ));
    }

    #[test]
    fn recognizes_sql_marker_at_literal_head() {
        assert!(string_literal_starts_with_sql_marker(
            r#""--sql
SELECT id FROM workers""#
        ));
        assert!(string_literal_starts_with_sql_marker(
            r##"r#"--sql
SELECT id FROM workers"#"##
        ));
        assert!(!string_literal_starts_with_sql_marker(
            r#""SELECT id FROM workers""#
        ));
    }

    #[test]
    fn recognizes_documented_inline_file_length_exceptions() {
        let marker = "rust-lints-file-length-exception";
        assert!(has_inline_file_length_exception_with_marker(
            "// rust-lints-file-length-exception: owner=platform-owner reason=generated-parser expires=2026-08-01\nfn main() {}",
            marker,
        ));
        assert!(!has_inline_file_length_exception_with_marker(
            "// rust-lints-file-length-exception: owner=platform-owner reason=missing-expiry\nfn main() {}",
            marker,
        ));
    }

    #[test]
    fn inline_exception_marker_is_configurable() {
        // A project supplying its own marker (e.g. a custom prefix) still works.
        assert!(has_inline_file_length_exception_with_marker(
            "// acme-file-length-exception: owner=o reason=r expires=2026-08-01\nfn main() {}",
            "acme-file-length-exception",
        ));
    }

    #[test]
    fn recognizes_inventory_file_length_exceptions() {
        let inventory = "path\towner\treason\texpires\tline_count\n\
             crates/platform-db/src/lib.rs\towner\treason\t2026-12-31\t900\n";
        assert!(inventory_lists_path(
            inventory,
            "crates/platform-db/src/lib.rs"
        ));
        assert!(!inventory_lists_path(
            inventory,
            "crates/platform-domain/src/lib.rs"
        ));
    }

    #[test]
    fn repo_source_filter_respects_configured_roots() {
        let roots = vec!["apps/".to_owned(), "crates/".to_owned()];
        assert!(is_repo_rust_source_path_with_roots(
            std::path::Path::new("crates/platform-db/src/lib.rs"),
            &roots,
        ));
        assert!(!is_repo_rust_source_path_with_roots(
            std::path::Path::new(
                "/nix/store/rust/lib/rustlib/src/rust/library/stdarch/crates/core_arch/src/x86/sse.rs"
            ),
            &roots,
        ));
    }

    #[test]
    fn recognizes_blocking_calls() {
        assert!(is_blocking_function_call("std::thread::sleep"));
        assert!(is_blocking_function_call("std::fs::read_to_string"));
        assert!(is_blocking_function_call(
            "tokio::runtime::Runtime::block_on"
        ));
        assert!(is_blocking_path("std::net::TcpStream::connect"));
        assert!(is_blocking_path("std::process::Command::output"));
        assert!(is_blocking_method_name("recv"));
        assert!(is_blocking_method_name("wait_with_output"));
        assert!(!is_blocking_function_call("tokio::time::sleep"));
        assert!(!is_blocking_path("std::process::id"));
        assert!(!is_blocking_method_name("try_recv"));
    }

    #[test]
    fn recognizes_blocking_quarantine_calls() {
        assert!(is_blocking_quarantine_call("tokio::task::spawn_blocking"));
        assert!(is_blocking_quarantine_call("spawn_blocking"));
        assert!(is_blocking_quarantine_call("tokio::task::block_in_place"));
        assert!(is_blocking_quarantine_call("std::thread::spawn"));
        assert!(!is_blocking_quarantine_call("tokio::spawn"));
    }

    #[test]
    fn recognizes_direct_reqwest_client_construction() {
        assert!(contains_direct_reqwest_client(
            "reqwest::Client::builder().build()"
        ));
        assert!(contains_direct_reqwest_client(
            "reqwest::blocking::Client::new()"
        ));
        assert!(!contains_direct_reqwest_client(
            "OutboundHttpClient::builder()"
        ));
    }

    #[test]
    fn recognizes_silent_saturation_shapes() {
        // Outer call must be one of the unwrap_or family.
        assert!(is_unwrap_or_family("unwrap_or"));
        assert!(is_unwrap_or_family("unwrap_or_default"));
        assert!(is_unwrap_or_family("unwrap_or_else"));
        assert!(!is_unwrap_or_family("unwrap"));
        assert!(!is_unwrap_or_family("expect"));
        // Receiver must be a fallible conversion.
        assert!(is_fallible_conversion_name("try_from"));
        assert!(is_fallible_conversion_name("try_into"));
        assert!(!is_fallible_conversion_name("from"));
        assert!(!is_fallible_conversion_name("parse"));
    }

    #[test]
    fn recognizes_unbounded_channel_paths() {
        assert!(is_unbounded_channel_path(
            "tokio::sync::mpsc::unbounded_channel"
        ));
        assert!(is_unbounded_channel_path(
            "futures::channel::mpsc::unbounded"
        ));
        assert!(is_unbounded_channel_path("crossbeam_channel::unbounded"));
        // A crate-prefixed re-export / fixture stand-in still matches on the
        // module-qualified suffix.
        assert!(is_unbounded_channel_path(
            "fixture_product_conversions::tokio::sync::mpsc::unbounded_channel"
        ));
        // A bounded constructor or an unrelated `unbounded`-named fn is fine.
        assert!(!is_unbounded_channel_path("tokio::sync::mpsc::channel"));
        assert!(!is_unbounded_channel_path("my_crate::unbounded"));
    }

    #[test]
    fn skips_setter_style_bool_param_names() {
        assert!(is_exempt_fn_name("set_enabled"));
        assert!(is_exempt_fn_name("with_retries"));
        assert!(!is_exempt_fn_name("render"));
        assert!(!is_exempt_fn_name("settle")); // not a `set_` setter
    }

    #[test]
    fn new_passes_are_off_by_default() {
        // Self-test sets all three vars; assert the toggles only read their own
        // var by clearing them explicitly for this assertion.
        // (Tests run single-threaded for env isolation.)
        for var in [
            "RUST_LINTS_SILENT_SATURATION",
            "RUST_LINTS_UNBOUNDED_CHANNEL",
            "RUST_LINTS_BOOL_PARAMS",
        ] {
            // SAFETY: tests are single-threaded; we restore nothing because the
            // default-off contract is what we are asserting.
            unsafe {
                std::env::remove_var(var);
            }
        }
        assert!(!config::silent_saturation_enabled());
        assert!(!config::unbounded_channel_enabled());
        assert!(!config::bool_params_enabled());
    }

    #[test]
    fn config_defaults_are_sensible() {
        // With no env set, owner lists and label fall back to built-in defaults.
        // (Tests run single-threaded for env isolation; defaults only assert the
        // shape, not exact contents, to avoid coupling to other tests' env.)
        assert!(!config::sqlx_owner_paths().is_empty());
        assert!(!config::http_owner_paths().is_empty());
        assert!(!config::repo_source_roots().is_empty());
    }
}
