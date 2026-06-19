#!/usr/bin/env bash
#
# Real-world build validation for hx.
#
# Drives the core loop (init/new -> build -> test -> run) against projects that
# pull real Hackage dependencies, not just `base`. This catches breakage that
# the base-only unit/e2e tests cannot — the kind of "works on a hello-world but
# not on a real project" gap.
#
# Usage:
#   HX=/path/to/hx ./scripts/real-world-test.sh
#
# Modes (env):
#   REAL_WORLD_QUICK=1   only the base-only scenario (fast local smoke)
#   REAL_WORLD_FULL=1    also the heavier templates (webapp, server, numeric)
#
# Exits non-zero if any scenario fails.

set -uo pipefail

HX="${HX:-hx}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
declare -a RESULTS=()

# run <label> <command...>
run() {
  local label="$1"
  shift
  echo "::group::${label}"
  if "$@"; then
    echo "PASS: ${label}"
    PASS=$((PASS + 1))
    RESULTS+=("PASS  ${label}")
  else
    local code=$?
    echo "FAIL: ${label} (exit ${code})"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL  ${label}")
  fi
  echo "::endgroup::"
}

echo "hx under test: $("$HX" --version 2>/dev/null || echo unknown)"
"$HX" doctor || true   # informational; never fatal

# Project names are prefixed to avoid colliding with real Hackage package
# names (e.g. a project literally named "base" conflicts with GHC's base).

# --- Scenario: base-only binary project ------------------------------------
BASE="$WORK/hxrw-app"
( cd "$WORK" && "$HX" init hxrw-app --name hxrw-app >/dev/null 2>&1 ) || true
run "base: build" bash -c "cd '$BASE' && '$HX' build"
run "base: run"   bash -c "cd '$BASE' && '$HX' run"
run "base: test"  bash -c "cd '$BASE' && '$HX' test"

# --- Scenario: hx new templates (real Hackage dependencies) ----------------
if [ "${REAL_WORLD_QUICK:-0}" != "1" ]; then
  templates=(cli library)
  if [ "${REAL_WORLD_FULL:-0}" = "1" ]; then
    templates+=(webapp server numeric)
  fi
  for tmpl in "${templates[@]}"; do
    name="hxrw-${tmpl}"
    proj="$WORK/$name"
    run "${tmpl}: new"   bash -c "cd '$WORK' && '$HX' new ${tmpl} ${name}"
    run "${tmpl}: build" bash -c "cd '$proj' && '$HX' build"
    # Libraries have no executable; only run binaries.
    if [ "$tmpl" != "library" ]; then
      run "${tmpl}: test" bash -c "cd '$proj' && '$HX' test"
    fi
  done
fi

# --- Summary ---------------------------------------------------------------
echo
echo "==================== real-world results ===================="
printf '  %s\n' "${RESULTS[@]}"
echo "  ---"
echo "  PASS=${PASS}  FAIL=${FAIL}"
echo "============================================================"

[ "$FAIL" -eq 0 ]
