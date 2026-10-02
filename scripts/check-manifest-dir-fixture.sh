#!/usr/bin/env bash
# Run the build-time CARGO_MANIFEST_DIR pass over fixtures/policy-violations and
# compare the reported sites against the expectation carried by the fixture sources.
#
# Same contract as check-blocking-in-async-fixture.sh: the expectation is DERIVED
# from trailing markers in the fixture files, and the comparison is a set
# difference in both directions, so lost coverage and false positives fail alike.
# Two markers, because the pass makes two different claims:
#
#   // manifest-dir: expect           the call's literal proves it read the variable
#   // manifest-dir: expect-consumed  another macro swallowed the literal, and the
#                                     crate reads the variable, so it cannot be ruled out
#
# The kind is part of the compared key: a proven read reported as merely unresolved
# (or the reverse) is a mismatch, not a pass.
#
# Every target is linted (--all-targets): a test that bakes the checkout is in scope.
#
# Run inside the dev shell:
#   nix develop -c scripts/check-manifest-dir-fixture.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$repo_root/fixtures/policy-violations"
proven_message='must not read `CARGO_MANIFEST_DIR` at build time'
consumed_message='consumed by another macro in a crate that reads `CARGO_MANIFEST_DIR` at build time'
unlocated_message="could not locate this crate's build-time \`CARGO_MANIFEST_DIR\` read"
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
    RUST_LINTS_BUILD_TIME_MANIFEST_DIR=1 \
        cargo dylint --all -- --workspace --all-targets
) >"$work/run.log" 2>&1 || run_status=$?

if [ "$run_status" -ne 0 ]; then
    echo "FAIL: the lint run exited $run_status; its diagnostics cannot be trusted." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

for crate in fixture-product-manifest-dir fixture-product-manifest-dir-clean; do
    if ! grep -Eq "(Checking|Compiling) $crate " "$work/run.log"; then
        echo "FAIL: $crate was not compiled, so the pass never ran on it." >&2
        tail -30 "$work/run.log" >&2
        exit 1
    fi
done

if grep -qF -- "$unlocated_message" "$work/run.log"; then
    echo "FAIL: the pass could not read rustc's expansion table:" >&2
    grep -F -A3 -- "$unlocated_message" "$work/run.log" >&2
    exit 1
fi

# `site kind` lines, from the `-->` line that follows each diagnostic.
reported_sites() {
    local message="$1" kind="$2"
    grep -F -A1 -- "$message" "$work/run.log" \
        | grep -- '-->' \
        | sed "s/ *--> //; s/:[0-9]*\$/ $kind/" || true
}
{
    reported_sites "$proven_message" expect
    reported_sites "$consumed_message" expect-consumed
} | sort -u >"$work/reported"

(
    cd "$fixtures"
    grep -rn --include='*.rs' -E '// manifest-dir: expect(-consumed)?$' crates apps \
        | sed -E 's/^([^:]*:[0-9]+):.*\/\/ manifest-dir: (expect(-consumed)?)$/\1 \2/' \
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
    echo "FAIL: reads that must be refused were not reported as marked (lost coverage):" >&2
    printf '  %s\n' "$missing" >&2
    status=1
fi

if [ -n "$unexpected" ]; then
    echo "FAIL: sites reported with no matching marker (false positives or wrong kind):" >&2
    printf '  %s\n' "$unexpected" >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "OK: $(wc -l <"$work/expected") expected build-time CARGO_MANIFEST_DIR sites reported, and no others."
fi

exit "$status"
