#!/usr/bin/env bash
# CI's networked tests. The whole suite runs once; any test that failed runs again on its own, once. A test that passes
# the second time is reported as flaky (a GitHub warning) and does not fail the release; one that fails twice does.
# Timing-sensitive call tests on a loaded shared runner fail now and then for reasons unrelated to the change, and a
# release should not hang on that, but every such failure stays visible in the run summary.
set -uo pipefail
cd "$(dirname "$0")/.."

log=$(mktemp)
cargo test -p dissent-core --test it -- --ignored --skip internet 2>&1 | tee "$log"
first=${PIPESTATUS[0]}

if [[ $first -ne 0 ]]; then
  # The names libtest lists under the last "failures:" heading. The first one holds each failure's output, which can
  # have indented lines of its own, so each heading starts the list again.
  mapfile -t failed < <(awk '/^failures:$/ {n=0; list=1; next} list && /^    [^ ]/ {names[n++]=$1} /^test result/ {list=0} END {for (i=0; i<n; i++) print names[i]}' "$log" | sort -u)
  if [[ ${#failed[@]} -eq 0 ]]; then
    echo "::error::the networked suite failed without a list of failed tests (exit $first)"
    exit "$first"
  fi
  # More than a handful failing at once is not flakiness.
  if [[ ${#failed[@]} -gt 3 ]]; then
    echo "::error::${#failed[@]} networked tests failed; not retrying"
    exit "$first"
  fi
  for t in "${failed[@]}"; do
    echo "Retrying $t once on its own"
    if cargo test -p dissent-core --test it -- --ignored --exact "$t"; then
      echo "::warning title=Flaky networked test::$t failed in the full run and passed when run again on its own"
    else
      echo "::error::$t failed twice"
      exit 1
    fi
  done
fi

cargo test -p dissent-relay --test ban_disconnect -- --ignored
