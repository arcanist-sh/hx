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

# run_clean <label> <command...>
#
# Like run(), but also fails when a command that should produce clean output
# emits WARN/ERROR diagnostics — even if it exits 0. This catches *silent
# degradation*: e.g. a constraint parser that warns-and-drops instead of
# erroring still exits 0, so a plain exit-code check reports a false PASS.
# Use only for commands expected to be quiet on a healthy project (resolution
# and metadata queries), not for build/test output.
run_clean() {
  local label="$1"
  shift
  echo "::group::${label}"
  local out code noise
  out="$("$@" 2>&1)"
  code=$?
  echo "$out"
  # Strip ANSI, then look for tracing WARN/ERROR levels or a CLI `error:` line.
  noise="$(printf '%s\n' "$out" \
    | sed -E 's/\x1b\[[0-9;]*m//g' \
    | grep -nE '(^|[[:space:]])(WARN|ERROR)[[:space:]]|^error:' || true)"
  if [ "$code" -ne 0 ]; then
    echo "FAIL: ${label} (exit ${code})"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL  ${label} (exit ${code})")
  elif [ -n "$noise" ]; then
    echo "FAIL: ${label} (exited 0 but emitted diagnostics):"
    printf '%s\n' "$noise" | head -10
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL  ${label} (noisy output)")
  else
    echo "PASS: ${label}"
    PASS=$((PASS + 1))
    RESULTS+=("PASS  ${label}")
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
    # BHC-backed templates (server, numeric) are skipped here regardless of
    # whether BHC is installed: BHC 0.2.3 cannot yet compile their source
    # (polymorphic numerics like `sum`/`fromIntegral` over Double, and Servant).
    # The hx -> BHC pipeline itself is covered by scripts/bhc-smoke.sh against a
    # program BHC can compile. Re-enable these once BHC gains those features.
    if grep -q 'backend = "bhc"' "$proj/hx.toml" 2>/dev/null; then
      echo "SKIP: ${tmpl}: build (BHC 0.2.3 cannot compile this template yet)"
      SKIP=$((SKIP + 1))
      RESULTS+=("SKIP  ${tmpl}: BHC compiler gap")
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

  # Metadata/resolution commands must be quiet on a healthy project, so they go
  # through run_clean (a stray WARN means a parser/resolver is silently
  # degrading). build/publish legitimately print progress, so use plain run.
  run       "cmd: check"        bash -c "cd '$CMDS' && '$HX' check"
  run_clean "cmd: add"          bash -c "cd '$CMDS' && '$HX' add containers"
  run       "cmd: build (+dep)" bash -c "cd '$CMDS' && '$HX' build"
  run_clean "cmd: lock"         bash -c "cd '$CMDS' && '$HX' lock"

  # The original empty-lockfile bug exited 0 with no warnings, so run_clean
  # alone would not catch it: assert the lockfile actually recorded packages.
  echo "::group::cmd: lock populated"
  if grep -q 'name = ' "$CMDS/hx.lock" 2>/dev/null; then
    echo "PASS: cmd: lock recorded resolved packages"
    PASS=$((PASS + 1))
    RESULTS+=("PASS  cmd: lock populated")
  else
    echo "FAIL: cmd: lock (hx.lock recorded no packages — empty resolution)"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL  cmd: lock empty")
  fi
  echo "::endgroup::"

  run_clean "cmd: sync"         bash -c "cd '$CMDS' && '$HX' sync"
  run_clean "cmd: tree"         bash -c "cd '$CMDS' && '$HX' tree"
  run_clean "cmd: outdated"     bash -c "cd '$CMDS' && '$HX' outdated"
  # why/deps read the lockfile; they only work once it is populated.
  run_clean "cmd: why"          bash -c "cd '$CMDS' && '$HX' why containers"
  run_clean "cmd: deps list"    bash -c "cd '$CMDS' && '$HX' deps list"
  run       "cmd: publish-dry"  bash -c "cd '$CMDS' && '$HX' publish --dry-run"
  run_clean "cmd: info"         bash -c "cd '$CMDS' && '$HX' info containers"

  # add -> rm round-trip: removing the dependency must actually drop it from
  # the .cabal file, not just exit 0.
  run_clean "cmd: rm"           bash -c "cd '$CMDS' && '$HX' rm containers"
  echo "::group::cmd: rm removed dependency"
  if grep -q 'containers' "$CMDS"/*.cabal 2>/dev/null; then
    echo "FAIL: cmd: rm (containers still present in .cabal after removal)"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL  cmd: rm did not remove dependency")
  else
    echo "PASS: cmd: rm removed the dependency from .cabal"
    PASS=$((PASS + 1))
    RESULTS+=("PASS  cmd: rm removed dependency")
  fi
  echo "::endgroup::"

  # clean -> rebuild: artifacts are removed and the project builds again.
  run "cmd: clean"           bash -c "cd '$CMDS' && '$HX' clean"
  run "cmd: rebuild (clean)" bash -c "cd '$CMDS' && '$HX' build"

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

# --- Scenario: adopt a real published library ------------------------------
# The truest "ready to adopt" signal: take a real package off Hackage (one we
# did NOT generate), adopt it with `hx import`, and lock + build + test it. The
# package ships only a bare `.cabal` (no cabal.project), which is the common
# single-package layout. Heavier, so it runs only in FULL mode and when cabal
# is available to fetch the source.
if [ "${REAL_WORLD_FULL:-0}" = "1" ] && command -v cabal >/dev/null 2>&1; then
  ADOPT="$WORK/adopt"
  mkdir -p "$ADOPT"
  if ( cd "$ADOPT" && cabal get optparse-applicative-0.18.1.0 >/dev/null 2>&1 ); then
    pkg="$ADOPT/optparse-applicative-0.18.1.0"
    # No cabal.project on purpose — exercises bare-.cabal adoption.
    run       "adopt: import"  bash -c "cd '$pkg' && '$HX' import --from cabal"
    run_clean "adopt: lock"    bash -c "cd '$pkg' && '$HX' lock"

    # Conditional evaluation: a non-Windows lockfile must NOT contain the
    # Windows-only `Win32` (pulled in by `process`'s `if os(windows)` branch).
    echo "::group::adopt: no Win32 leak"
    if grep -qi 'name = "Win32"' "$pkg/hx.lock" 2>/dev/null; then
      echo "FAIL: adopt: Win32 leaked into the lockfile on a non-Windows host"
      FAIL=$((FAIL + 1)); RESULTS+=("FAIL  adopt: Win32 leak")
    else
      echo "PASS: adopt: no Windows-only deps in the lockfile"
      PASS=$((PASS + 1)); RESULTS+=("PASS  adopt: no Win32 leak")
    fi
    echo "::endgroup::"

    run       "adopt: build"   bash -c "cd '$pkg' && '$HX' build"
    run       "adopt: test"    bash -c "cd '$pkg' && '$HX' test"
  else
    echo "SKIP: adopt (could not fetch optparse-applicative from Hackage)"
    SKIP=$((SKIP + 1)); RESULTS+=("SKIP  adopt: fetch failed")
  fi

  # --- Scenario: adopt a multi-component package ---------------------------
  # pretty-simple has a library, several executables, a test-suite and a
  # benchmark, with flags that gate whole components via `buildable:`. It
  # stresses component handling and conditional/flag evaluation together.
  MC="$WORK/mc"
  mkdir -p "$MC"
  if ( cd "$MC" && cabal get pretty-simple-4.1.2.0 >/dev/null 2>&1 ); then
    mcpkg="$MC/pretty-simple-4.1.2.0"
    run       "multi: import" bash -c "cd '$mcpkg' && '$HX' import --from cabal"
    run_clean "multi: lock"   bash -c "cd '$mcpkg' && '$HX' lock"

    # `buildexample` defaults off, disabling the JSON example via
    # `buildable: False`; its `aeson` dependency must not enter the lockfile.
    echo "::group::multi: disabled-component dep excluded"
    if grep -qi 'name = "aeson"' "$mcpkg/hx.lock" 2>/dev/null; then
      echo "FAIL: multi: aeson leaked from a buildable:False component"
      FAIL=$((FAIL + 1)); RESULTS+=("FAIL  multi: buildable leak")
    else
      echo "PASS: multi: disabled-component deps excluded"
      PASS=$((PASS + 1)); RESULTS+=("PASS  multi: no buildable leak")
    fi
    echo "::endgroup::"

    run       "multi: build"  bash -c "cd '$mcpkg' && '$HX' build"
  else
    echo "SKIP: multi (could not fetch pretty-simple from Hackage)"
    SKIP=$((SKIP + 1)); RESULTS+=("SKIP  multi: fetch failed")
  fi

  # --- Scenario: a multi-package workspace --------------------------------
  # A cabal.project with two local packages (a library and an app that depends
  # on it). There is no package in the root directory, so build/test must use
  # the `all` target — an untargeted cabal invocation fails with Cabal-7134.
  WS="$WORK/workspace"
  mkdir -p "$WS/wslib/src" "$WS/wsapp/app"
  cat >"$WS/cabal.project" <<'EOF'
packages: wslib wsapp
EOF
  cat >"$WS/wslib/wslib.cabal" <<'EOF'
cabal-version: 3.0
name: wslib
version: 0.1.0.0
build-type: Simple
library
  exposed-modules: WsLib
  hs-source-dirs: src
  build-depends: base
  default-language: Haskell2010
EOF
  printf 'module WsLib (hello) where\nhello :: String\nhello = "from wslib"\n' >"$WS/wslib/src/WsLib.hs"
  cat >"$WS/wsapp/wsapp.cabal" <<'EOF'
cabal-version: 3.0
name: wsapp
version: 0.1.0.0
build-type: Simple
executable wsapp
  main-is: Main.hs
  hs-source-dirs: app
  build-depends: base, wslib
  default-language: Haskell2010
test-suite wsapp-test
  type: exitcode-stdio-1.0
  main-is: Test.hs
  hs-source-dirs: app
  build-depends: base, wslib
  default-language: Haskell2010
EOF
  printf 'module Main where\nimport WsLib (hello)\nmain :: IO ()\nmain = putStrLn hello\n' >"$WS/wsapp/app/Main.hs"
  printf 'module Main where\nimport WsLib (hello)\nmain :: IO ()\nmain = if null hello then error "fail" else putStrLn "ok"\n' >"$WS/wsapp/app/Test.hs"

  run       "workspace: import" bash -c "cd '$WS' && '$HX' import --from cabal"
  run_clean "workspace: lock"   bash -c "cd '$WS' && '$HX' lock"
  run       "workspace: build"  bash -c "cd '$WS' && '$HX' build"
  run       "workspace: test"   bash -c "cd '$WS' && '$HX' test"
fi

# --- Summary ---------------------------------------------------------------
echo
echo "==================== real-world results ===================="
printf '  %s\n' "${RESULTS[@]}"
echo "  ---"
echo "  PASS=${PASS}  FAIL=${FAIL}  SKIP=${SKIP}"
echo "============================================================"

[ "$FAIL" -eq 0 ]
