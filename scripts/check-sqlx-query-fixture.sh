#!/usr/bin/env bash
# Run the two SQLx runtime-query passes over fixtures/policy-violations and compare the
# reported sites against the expectation carried by the fixture sources.
#
# Same contract as check-blocking-in-async-fixture.sh: the expectation is DERIVED
# from trailing markers in the fixture files, and the comparison is a set
# difference in both directions, so lost coverage and false positives fail alike.
# Two markers, one per pass:
#
#   // sqlx-query: expect-static   static SQL handed to sqlx::query/query_as/query_scalar
#                                  (the strict compile-time-macro pass)
#   // sqlx-query: expect-dynamic  dynamic SQL with no documented safety boundary
#                                  (the dynamic-SQL pass, which runs in the SQL seam only)
#
# The kind is part of the compared key: a call reported by the wrong pass is a
# mismatch, not a pass. `crates/db-seam/` is named the SQL seam so the dynamic pass
# runs there; every other crate stays outside it.
#
# Run inside the dev shell:
#   nix develop -c scripts/check-sqlx-query-fixture.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$repo_root/fixtures/policy-violations"
static_message='static SQLx queries should use compile-time checked macros'
dynamic_message='dynamic SQLx query constructors must cross a documented SQL safety boundary'
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

lib="${DYLINT_LIBRARY_PATH:-}"
if [ -z "$lib" ]; then
    lib="$(nix build "$repo_root#default" --no-link --print-out-paths)/lib"
fi

# A stale fixture target dir replays cached verdicts and reports nothing, which
# reads exactly like "the pass found no violations". Always lint from cold.
fixture_target="$work/target"

# Warn mode: a finding is not an error, so the run must succeed. A run that
# emitted every expected diagnostic and then died leaves a log that parses like
# a clean one, so its status is checked rather than discarded.
run_status=0
(
    cd "$fixtures"
    CARGO_TARGET_DIR="$fixture_target" \
    CARGO_INCREMENTAL=0 \
    DYLINT_LIBRARY_PATH="$lib" \
    RUST_LINTS_STRICT_SQLX=1 \
    RUST_LINTS_SQLX_OWNER_PATHS=crates/db-seam/ \
        cargo dylint --all -- --workspace
) >"$work/run.log" 2>&1 || run_status=$?

if [ "$run_status" -ne 0 ]; then
    echo "FAIL: the lint run exited $run_status; its diagnostics cannot be trusted." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

for crate in fixture-db-seam fixture-product-core fixture-policy-api; do
    if ! grep -Eq "(Checking|Compiling) $crate " "$work/run.log"; then
        echo "FAIL: $crate was not compiled, so the passes never ran on it." >&2
        tail -30 "$work/run.log" >&2
        exit 1
    fi
done

# `site kind` lines, from the `-->` line that follows each diagnostic.
reported_sites() {
    local message="$1" kind="$2"
    grep -F -A1 -- "$message" "$work/run.log" \
        | grep -- '-->' \
        | sed "s/ *--> //; s/:[0-9]*\$/ $kind/" || true
}
{
    reported_sites "$static_message" expect-static
    reported_sites "$dynamic_message" expect-dynamic
} | sort -u >"$work/reported"

(
    cd "$fixtures"
    grep -rn --include='*.rs' -E '// sqlx-query: expect-(static|dynamic)$' crates apps \
        | sed -E 's/^([^:]*:[0-9]+):.*\/\/ sqlx-query: (expect-(static|dynamic))$/\1 \2/' \
        | sort -u
) >"$work/expected"

if [ ! -s "$work/expected" ]; then
    echo "FAIL: no expectation marker found in the fixtures; the comparison would prove nothing." >&2
    exit 1
fi

missing="$(comm -23 "$work/expected" "$work/reported")"
unexpected="$(comm -13 "$work/expected" "$work/reported")"
status=0

if [ -n "$missing" ]; then
    echo "FAIL: calls that must be reported were not reported as marked (lost coverage):" >&2
    printf '  %s\n' "$missing" >&2
    status=1
fi

if [ -n "$unexpected" ]; then
    echo "FAIL: calls reported with no matching marker (false positives or wrong pass):" >&2
    printf '  %s\n' "$unexpected" >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "OK: $(wc -l <"$work/expected") expected SQLx runtime-query sites reported, and no others."
fi

exit "$status"
