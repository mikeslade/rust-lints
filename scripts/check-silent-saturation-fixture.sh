#!/usr/bin/env bash
# Run the silent-saturation pass over fixtures/policy-violations and compare the
# reported sites against the expectation carried by the fixture sources.
#
# Same contract as check-blocking-in-async-fixture.sh: every fixture line that must
# be flagged ends with a `// silent-saturation: expect` marker, and the
# comparison is a set difference in both directions, so lost coverage and false
# positives fail alike.
#
# Every target is linted (--all-targets), so the `#[cfg(test)]` conversions in the
# fixture prove the pass still skips the test-crate build.
#
# Run inside the dev shell:
#   nix develop -c scripts/check-silent-saturation-fixture.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$repo_root/fixtures/policy-violations"
message='numeric conversion silently saturates/substitutes a default on overflow'
marker='// silent-saturation: expect'
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
    RUST_LINTS_SILENT_SATURATION=1 \
        cargo dylint --all -- --workspace --all-targets
) >"$work/run.log" 2>&1 || run_status=$?

if [ "$run_status" -ne 0 ]; then
    echo "FAIL: the lint run exited $run_status; its diagnostics cannot be trusted." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

if ! grep -Eq '(Checking|Compiling) fixture-product-conversions ' "$work/run.log"; then
    echo "FAIL: fixture-product-conversions was not compiled, so the pass never ran on it." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

grep -F -A1 -- "$message" "$work/run.log" \
    | grep -- '-->' \
    | sed 's/ *--> //; s/:[0-9]*$//' \
    | sort -u >"$work/reported" || true

(
    cd "$fixtures"
    grep -rn --include='*.rs' -E "$marker\$" crates apps \
        | cut -d: -f1,2 \
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
    echo "FAIL: conversions that must be flagged were not reported (lost coverage):" >&2
    printf '  %s\n' $missing >&2
    status=1
fi

if [ -n "$unexpected" ]; then
    echo "FAIL: sites flagged with no expectation marker (false positives):" >&2
    printf '  %s\n' $unexpected >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "OK: $(wc -l <"$work/expected") expected silent-saturation sites reported, and no others."
fi

exit "$status"
