# rust-lints

Architecture-policy Rust lints as a [Dylint](https://github.com/trailofbits/dylint)
library — the mechanical enforcement for rules Clippy cannot express:

- **SQL-seam ownership** — SQL lives only in the persistence-seam crate.
- **Inline-SQL markers** — every inline SQL literal starts with `--sql`.
- **Compile-time SQLx** — static SQL handed to `sqlx::query`, `query_as` or
  `query_scalar` should use the checked macros (`RUST_LINTS_STRICT_SQLX=1`), and
  dynamic SQL in the seam must cross a documented safety boundary. The callee is
  judged by the definition it resolved to, so a turbofish or a renamed import is
  the same call, and the SQL argument is followed to its definition: a literal, a
  `const` in any module or crate, a `static`, or a function returning one.
- **Outbound-HTTP wrapper** — HTTP goes through the reviewed wrapper crate,
  not a raw client (`RUST_LINTS_HTTP_WRAPPER=1`).
- **Blocking-in-async quarantine** — blocking calls in async contexts get
  flagged for `spawn_blocking` (`RUST_LINTS_BLOCKING_TOKIO=1`). A call is
  blocking by its resolved path, or by its method name when the value it
  produces is not a `Future` — so a project's own wrapper around
  `std::sync::mpsc` is caught while Tokio's async `recv`/`wait` are not.
- **Silent saturation** — `saturating_*` arithmetic needs a documented
  business rule (`RUST_LINTS_SILENT_SATURATION=1`).
- **Unbounded channels** — `RUST_LINTS_UNBOUNDED_CHANNEL=1`.
- **Boolean positional parameters** — `RUST_LINTS_BOOL_PARAMS=1`.
- **Build-time `CARGO_MANIFEST_DIR` reads** — an `env!`/`option_env!` that
  reads `CARGO_MANIFEST_DIR` bakes in the checkout the crate was compiled in
  (`RUST_LINTS_BUILD_TIME_MANIFEST_DIR=1`). Judged after expansion: the macro
  by the definition it resolved to, the variable by the name rustc recorded
  reading, so a renamed `env` or a macro-built name changes nothing. Tests,
  benches, examples and build scripts are all in scope.
- **File-length ratchet** — `RUST_LINTS_MAX_FILE_LINES=<n>`, with a
  `rust-lints-file-length-exception` marker escape hatch
  (see `file-length-exceptions.tsv`).

All passes are gated/configured by `RUST_LINTS_*` env vars; suppress a
finding in code with `#[allow(rust_lints_policy_checks)]` plus a reason.

## Consuming (nix)

```nix
inputs.rust-lints.url = "github:mikeslade/rust-lints";
```

```just
dylint-check:
    PATH="$PWD/scripts/dylint-shim:$PATH" \
    DYLINT_LIBRARY_PATH="$(nix build .#rust-lints --print-out-paths 2>/dev/null || nix build github:mikeslade/rust-lints --print-out-paths)/lib" \
    RUST_LINTS_MAX_FILE_LINES=800 RUST_LINTS_REQUIRE_SQL_MARKER=1 RUST_LINTS_STRICT_SQLX=1 \
    RUST_LINTS_HTTP_WRAPPER=1 RUST_LINTS_BLOCKING_TOKIO=1 \
    RUST_LINTS_SILENT_SATURATION=1 RUST_LINTS_UNBOUNDED_CHANNEL=1 RUST_LINTS_BOOL_PARAMS=1 \
    RUST_LINTS_BUILD_TIME_MANIFEST_DIR=1 \
    cargo dylint --all -- --workspace
```

The library targets a rolling `rustc_private` nightly. `rust-toolchain` selects
`nightly` for rustup users; the flake independently selects the newest nightly
with all required components in the locked rust-overlay snapshot, builds it
hermetically via crane, and exposes:

- `packages.default` (alias `packages.dylints`) — the cdylib with the
  toolchain-suffixed symlink `cargo dylint --no-build` resolves
- `packages.toolchain` — the resolved nightly, for building via a local shim
- `devShells.default` — toolchain + cargo-dylint + dylint-link
  (`direnv allow` loads it automatically via the checked-in `.envrc`)

The flake derives an exact dated toolchain name from that rolling selection for
the cdylib suffix and `RUSTUP_TOOLCHAIN`. Consuming shims must report that same
name so Dylint neither misses the library nor reuses a driver from another
nightly. The date-free `rust-toolchain` entry is for editors and general rustup
commands; run Dylint through the Nix shell, which supplies the exact identity.

## Fixtures

`fixtures/policy-violations/` is a workspace where each pass has a VIOLATION
case and a compliant case, for exercising the lints against a known corpus:

```bash
nix develop -c bash -c 'cd fixtures/policy-violations && \
  DYLINT_LIBRARY_PATH=$(nix build ..#default --print-out-paths)/lib \
  RUST_LINTS_MAX_FILE_LINES=100 RUST_LINTS_REQUIRE_SQL_MARKER=1 \
  cargo dylint --all -- --workspace'
```

The blocking-in-async cases are asserted rather than eyeballed:

```bash
nix develop -c scripts/check-blocking-in-async-fixture.sh
```

Each fixture line that must be flagged carries a trailing
`// blocking-in-async: expect` marker, and the script set-diffs the markers
against what the pass actually reported, in both directions. A change that
removes a false positive by removing coverage fails it as loudly as one that
introduces a false positive.

The build-time `CARGO_MANIFEST_DIR` cases are asserted the same way, over every
target (`--all-targets`), with `// manifest-dir: expect` and
`// manifest-dir: expect-consumed` markers:

```bash
nix develop -c scripts/check-manifest-dir-fixture.sh
```

The SQLx runtime-query cases are asserted with `// sqlx-query: expect-static` and
`// sqlx-query: expect-dynamic` markers. `crates/sqlx` and `crates/sqlx-core` are
stand-ins with the real crates' names and item paths:

```bash
nix develop -c scripts/check-sqlx-query-fixture.sh
```
