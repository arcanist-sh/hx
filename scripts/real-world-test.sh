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
SKIP=0
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

# Detect whether the BHC backend is available. BHC-backed templates (server,
# numeric) are skipped — not failed — when BHC is not installed.
BHC_OK=0
if "$HX" doctor 2>&1 | grep -qE 'bhc: [0-9]'; then BHC_OK=1; fi

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
    # BHC-backed templates need the BHC compiler; skip when it isn't installed.
    if grep -q 'backend = "bhc"' "$proj/hx.toml" 2>/dev/null && [ "$BHC_OK" != "1" ]; then
      echo "SKIP: ${tmpl}: build (requires the BHC backend, not installed)"
      SKIP=$((SKIP + 1))
      RESULTS+=("SKIP  ${tmpl}: requires BHC")
      continue
    fi
    run "${tmpl}: build" bash -c "cd '$proj' && '$HX' build"
    # Libraries have no executable; only run binaries.
    if [ "$tmpl" != "library" ]; then
      run "${tmpl}: test" bash -c "cd '$proj' && '$HX' test"
    fi
  done
fi

# --- Scenario: daily-driver commands on a real project ---------------------
# Runs against a scaffolded cli project (real Hackage dependencies) to cover
# the commands a user touches every day, beyond `new`/`build`.
if [ "${REAL_WORLD_QUICK:-0}" != "1" ]; then
  CMDS="$WORK/hxrw-cmds"
  ( cd "$WORK" && "$HX" new cli hxrw-cmds >/dev/null 2>&1 ) || true

  run "cmd: check"        bash -c "cd '$CMDS' && '$HX' check"
  run "cmd: add"          bash -c "cd '$CMDS' && '$HX' add containers"
  run "cmd: build (+dep)" bash -c "cd '$CMDS' && '$HX' build"
  run "cmd: lock"         bash -c "cd '$CMDS' && '$HX' lock"
  run "cmd: sync"         bash -c "cd '$CMDS' && '$HX' sync"
  run "cmd: tree"         bash -c "cd '$CMDS' && '$HX' tree"
  run "cmd: outdated"     bash -c "cd '$CMDS' && '$HX' outdated"
  run "cmd: publish-dry"  bash -c "cd '$CMDS' && '$HX' publish --dry-run"

  # fmt/lint need fourmolu/hlint; skip (not fail) when they aren't installed.
  if command -v fourmolu >/dev/null 2>&1; then
    run "cmd: fmt"        bash -c "cd '$CMDS' && '$HX' fmt"
  else
    echo "SKIP: cmd: fmt (fourmolu not installed)"
    SKIP=$((SKIP + 1)); RESULTS+=("SKIP  cmd: fmt (no fourmolu)")
  fi
  if command -v hlint >/dev/null 2>&1; then
    run "cmd: lint"       bash -c "cd '$CMDS' && '$HX' lint"
  else
    echo "SKIP: cmd: lint (hlint not installed)"
    SKIP=$((SKIP + 1)); RESULTS+=("SKIP  cmd: lint (no hlint)")
  fi
fi

# --- Summary ---------------------------------------------------------------
echo
echo "==================== real-world results ===================="
printf '  %s\n' "${RESULTS[@]}"
echo "  ---"
echo "  PASS=${PASS}  FAIL=${FAIL}  SKIP=${SKIP}"
echo "============================================================"

[ "$FAIL" -eq 0 ]
