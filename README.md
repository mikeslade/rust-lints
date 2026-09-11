# rust-lints

Architecture-policy Rust lints as a [Dylint](https://github.com/trailofbits/dylint)
library — the mechanical enforcement for rules Clippy cannot express:

- **SQL-seam ownership** — SQL lives only in the persistence-seam crate.
- **Inline-SQL markers** — every inline SQL literal starts with `--sql`.
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
- **File-length ratchet** — `RUST_LINTS_MAX_FILE_LINES=<n>`, with a
  `rust-lints-file-length-exception` marker escape hatch
  (see `file-length-exceptions.tsv`).
- **Closed-trait impls** — one nominated trait may only be implemented by the
  types named in an allowed set (`RUST_LINTS_CLOSED_TRAIT_IMPLS=my_crate::Trait`,
  `RUST_LINTS_CLOSED_TRAIT_ALLOWED_IMPLS=my_crate::A,my_crate::B`). Membership is
  decided on the resolved `DefId` of the trait ref and of the self type's ADT, so
  a nested generic bound, an import alias, a re-export and a `macro_rules!`
  expansion all arrive as the same two nodes. No source text is read.

  **What it holds, and what it does not.** Dylint visits only the crates the
  compilation builds, so an implementation in a crate outside the linted
  workspace is never presented to the pass and is never reported. What this pass
  establishes is *no implementation inside the linted workspace outside the
  allowed set*. A project whose trait is public and unsealed still has a foreign
  implementation open to it, and should say so where it states the trait's
  guarantee rather than calling the trait closed.

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

The closed-trait cases are asserted the same way:

```bash
nix develop -c scripts/check-closed-trait-impl-fixture.sh
```

Its marker is `// closed-trait-impl: expect`. The fixtures spell one trait three
ways a regex over `impl` headers cannot follow: a bound carrying nested angle
brackets, an import alias, and an impl that exists only after macro expansion.
A pass that stopped resolving `DefId`s reads as lost coverage. A fourth
implementor sits in the allowed set carrying no marker, so a pass that dropped
the allowed-set filter and began flagging every impl reads as a false positive.
