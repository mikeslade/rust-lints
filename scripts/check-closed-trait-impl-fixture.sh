#!/usr/bin/env bash
# Run the closed-trait-impl pass over fixtures/policy-violations and compare the
# reported sites against the expectation carried by the fixture sources.
#
# The expectation is DERIVED, not stored: every fixture line that must be flagged
# carries a trailing `// closed-trait-impl: expect` marker, so the two sets are
# read from the same file and a fixture edit cannot drift from a checked-in list.
# The comparison is a set difference in BOTH directions, which is the point. The
# fixtures spell one trait three ways the old regex inventory could not follow —
# a nested generic bound, an import alias, a macro expansion — so a pass that
# stopped resolving `DefId`s shows up here as lost coverage. And one compliant
# implementor sits in the allowed set with no marker, so a pass that dropped the
# allowed-set filter and started flagging every impl shows up as a false
# positive. Neither direction can be made green by weakening the other.
#
# Run inside the dev shell:
#   nix develop -c scripts/check-closed-trait-impl-fixture.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$repo_root/fixtures/policy-violations"
marker='// closed-trait-impl: expect'
closed_trait='fixture_product_core::OutboxRepository'
allowed_impls='fixture_product_integration::ReviewedWriter'
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

lib="${DYLINT_LIBRARY_PATH:-}"
if [ -z "$lib" ]; then
    lib="$(nix build "$repo_root#default" --no-link --print-out-paths)/lib"
fi

# A stale fixture target dir replays cached verdicts and reports nothing, which
# reads exactly like "the pass found no violations". Always lint from cold.
fixture_target="$work/target"

# The pass runs in WARN mode here, so a violation is not an error and the run
# must succeed. Its status is captured rather than discarded: a run that emitted
# every expected diagnostic and then died — a rustc ICE, a link failure, ENOSPC
# — leaves a log that parses exactly like a clean one.
run_status=0
(
    cd "$fixtures"
    CARGO_TARGET_DIR="$fixture_target" \
    CARGO_INCREMENTAL=0 \
    DYLINT_LIBRARY_PATH="$lib" \
    RUST_LINTS_CLOSED_TRAIT_IMPLS="$closed_trait" \
    RUST_LINTS_CLOSED_TRAIT_ALLOWED_IMPLS="$allowed_impls" \
        cargo dylint --all -- --workspace
) >"$work/run.log" 2>&1 || run_status=$?

if [ "$run_status" -ne 0 ]; then
    echo "FAIL: the lint run exited $run_status; its diagnostics cannot be trusted." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

if ! grep -q 'Checking fixture-product-integration' "$work/run.log"; then
    echo "FAIL: the fixture workspace was not compiled, so the pass never ran." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

# The `|| true` is what makes a pass that reports NOTHING legible. Under
# `set -e` and `pipefail` an empty grep would kill this script with a bare
# exit 1 — red, but silently, and a silent red is the state a reader most needs
# spelled out: it is exactly what losing the trait resolution looks like.
{
    grep -A1 'which is not in the allowed set' "$work/run.log" \
        | grep -- '-->' \
        | sed 's/ *--> //; s/:[0-9]*$//' \
        | sort -u
} >"$work/reported" || true

(
    cd "$fixtures"
    grep -rn --include='*.rs' -F "$marker" crates apps \
        | cut -d: -f1,2 \
        | sort -u
) >"$work/expected"

# An empty expectation set would make the set difference vacuously green, so the
# fixtures having lost their markers is its own failure rather than a pass.
if [ ! -s "$work/expected" ]; then
    echo "FAIL: no fixture line carries '$marker', so this script proves nothing." >&2
    exit 1
fi

missing="$(comm -23 "$work/expected" "$work/reported")"
unexpected="$(comm -13 "$work/expected" "$work/reported")"
status=0

if [ -n "$missing" ]; then
    echo "FAIL: closed-trait impls that must be flagged were not reported (lost coverage):" >&2
    printf '  %s\n' $missing >&2
    status=1
fi

if [ -n "$unexpected" ]; then
    echo "FAIL: impls flagged with no expectation marker (false positives):" >&2
    printf '  %s\n' $unexpected >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "OK: $(wc -l <"$work/expected") expected closed-trait impls reported, and no others."
fi

exit "$status"
