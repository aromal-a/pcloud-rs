#!/usr/bin/env bash
# DR drill driver. Runs every scenarios/*.sh, aggregates exit
# codes, and emits a grep-friendly summary the CI workflow can
# parse.
#
# Exit code:
#   0  - all scenarios PASS or SKIP.
#   1  - at least one FAIL.
#
# Plan reference: CLAUDEREV/TIER-PROGRESS.md row T4.2.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scenarios_dir="$here/scenarios"

if [ ! -d "$scenarios_dir" ]; then
    printf '[FAIL] dr_drill_runner: %s missing\n' "$scenarios_dir"
    exit 1
fi

shopt -s nullglob
scripts=("$scenarios_dir"/*.sh)
# Drop _common.sh helper; only run real scenarios.
real=()
for s in "${scripts[@]}"; do
    case "$(basename "$s")" in
        _*) ;;
        *) real+=("$s") ;;
    esac
done

if [ "${#real[@]}" -eq 0 ]; then
    printf '[FAIL] dr_drill_runner: no scenarios found under %s\n' "$scenarios_dir"
    exit 1
fi

pass=0
fail=0
skip=0
fail_names=()
skip_names=()

for s in "${real[@]}"; do
    name="$(basename "$s" .sh)"
    printf '\n=== running scenario: %s ===\n' "$name"
    set +e
    bash "$s"
    rc=$?
    set -e
    case "$rc" in
        0)  pass=$((pass + 1)) ;;
        77) skip=$((skip + 1)); skip_names+=("$name") ;;
        *)  fail=$((fail + 1)); fail_names+=("$name") ;;
    esac
done

printf '\n=== DR DRILL SUMMARY ===\n'
printf 'PASS=%d FAIL=%d SKIP=%d (total=%d)\n' \
    "$pass" "$fail" "$skip" "${#real[@]}"
if [ "$skip" -gt 0 ]; then
    printf 'skipped: %s\n' "${skip_names[*]}"
fi
if [ "$fail" -gt 0 ]; then
    printf 'failed:  %s\n' "${fail_names[*]}"
    exit 1
fi
exit 0
