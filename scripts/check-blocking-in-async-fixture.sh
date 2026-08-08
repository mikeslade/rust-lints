#!/usr/bin/env bash
# Run the blocking-in-async pass over fixtures/policy-violations and compare the
# reported sites against the expectation carried by the fixture sources.
#
# The expectation is DERIVED, not stored: every fixture line that must be flagged
# carries a trailing `// blocking-in-async: expect` marker, so the two sets are
# read from the same file and a fixture edit cannot drift from a checked-in list.
# The comparison is a set difference in BOTH directions, which is the point — a
# change that removes false positives by removing coverage fails here just as
# loudly as one that adds false positives.
#
# Run inside the dev shell:
#   nix develop -c scripts/check-blocking-in-async-fixture.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$repo_root/fixtures/policy-violations"
marker='// blocking-in-async: expect'
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
    RUST_LINTS_BLOCKING_TOKIO=1 \
        cargo dylint --all -- --workspace
) >"$work/run.log" 2>&1 || run_status=$?

if [ "$run_status" -ne 0 ]; then
    echo "FAIL: the lint run exited $run_status; its diagnostics cannot be trusted." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

if ! grep -q 'Checking fixture-product-async' "$work/run.log"; then
    echo "FAIL: the fixture workspace was not compiled, so the pass never ran." >&2
    tail -30 "$work/run.log" >&2
    exit 1
fi

grep -A1 'blocking calls must not run on Tokio async worker threads' "$work/run.log" \
    | grep -- '-->' \
    | sed 's/ *--> //; s/:[0-9]*$//' \
    | sort -u >"$work/reported"

(
    cd "$fixtures"
    grep -rn --include='*.rs' -F "$marker" crates apps \
        | cut -d: -f1,2 \
        | sort -u
) >"$work/expected"

missing="$(comm -23 "$work/expected" "$work/reported")"
unexpected="$(comm -13 "$work/expected" "$work/reported")"
status=0

if [ -n "$missing" ]; then
    echo "FAIL: blocking calls that must be flagged were not reported (lost coverage):" >&2
    printf '  %s\n' $missing >&2
    status=1
fi

if [ -n "$unexpected" ]; then
    echo "FAIL: sites flagged with no expectation marker (false positives):" >&2
    printf '  %s\n' $unexpected >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "OK: $(wc -l <"$work/expected") expected blocking sites reported, and no others."
fi

exit "$status"
