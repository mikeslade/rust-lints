# rust-lints

Architecture-policy Rust lints as a [Dylint](https://github.com/trailofbits/dylint)
library — the mechanical enforcement for rules Clippy cannot express
(extracted from review-kit; see the rust-conventions workspace reference for
the rules' rationale):

- **SQL-seam ownership** — SQL lives only in the persistence-seam crate.
- **Inline-SQL markers** — every inline SQL literal starts with `--sql`.
- **Outbound-HTTP wrapper** — HTTP goes through the reviewed wrapper crate,
  not a raw client (`RUST_LINTS_HTTP_WRAPPER=1`).
- **Blocking-in-async quarantine** — blocking calls in async contexts get
  flagged for `spawn_blocking` (`RUST_LINTS_BLOCKING_TOKIO=1`).
- **Silent saturation** — `saturating_*` arithmetic needs a documented
  business rule (`RUST_LINTS_SILENT_SATURATION=1`).
- **Unbounded channels** — `RUST_LINTS_UNBOUNDED_CHANNEL=1`.
- **Boolean positional parameters** — `RUST_LINTS_BOOL_PARAMS=1`.
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
    cargo dylint --all -- --workspace
```

The library links against a pinned `rustc_private` nightly (see
`rust-toolchain`); the flake builds it hermetically via crane and exposes:

- `packages.default` (alias `packages.dylints`) — the cdylib with the
  toolchain-suffixed symlink `cargo dylint --no-build` resolves
- `packages.toolchain` — the pinned nightly, for building via a local shim
- `devShells.default` — toolchain + cargo-dylint + dylint-link
  (`direnv allow` loads it automatically via the checked-in `.envrc`)

## Migrating from review-kit's `review_kit_lints`

Mechanical renames in the consuming repo:

- flake input `review-kit` → `rust-lints`; attr `.#dylints` still works
- env prefix `RK_DYLINT_` → `RUST_LINTS_`
- `#[allow(review_kit_policy_checks)]` → `#[allow(rust_lints_policy_checks)]`
- markers `review-kit-file-length-exception` → `rust-lints-file-length-exception`,
  `review-kit-dynamic-sql` → `rust-lints-dynamic-sql`

## Fixtures

`fixtures/policy-violations/` is a workspace where each pass has a VIOLATION
case and a compliant case, for exercising the lints against a known corpus:

```bash
nix develop -c bash -c 'cd fixtures/policy-violations && \
  DYLINT_LIBRARY_PATH=$(nix build ..#default --print-out-paths)/lib \
  RUST_LINTS_MAX_FILE_LINES=100 RUST_LINTS_REQUIRE_SQL_MARKER=1 \
  cargo dylint --all -- --workspace'
```
