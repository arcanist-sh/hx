#!/usr/bin/env bash
#
# BHC pipeline smoke test for hx.
#
# Proves the hx -> BHC build pipeline end to end against a real BHC toolchain:
# detection, the correct CLI invocation, LIBRARY_PATH wiring for the runtime
# libraries, linking, honest failure reporting (older BHC releases exit 0
# even on errors, so hx must not trust the exit status alone), and `hx test`
# building and running a cabal test-suite (pass and fail).
#
# It deliberately uses a *minimal* program, so a failure points at the hx
# side of the pipeline rather than at a BHC language-coverage gap. The
# `numeric` and `server` templates are NOT exercised here: they depend on
# polymorphic numeric code and Servant, which are BHC feature work tracked in
# the BHC issue tracker, not part of the hx contract this script checks.
#
# Usage:
#   HX=/path/to/hx ./scripts/bhc-smoke.sh
#
# Requires `bhc` on PATH (the CI job installs it first).

set -uo pipefail

HX="${HX:-hx}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0

ok()   { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad()  { echo "FAIL: $1"; FAIL=$((FAIL + 1)); }

echo "hx:  $("$HX" --version 2>/dev/null || echo unknown)"
if ! command -v bhc >/dev/null 2>&1; then
  echo "FAIL: bhc not found on PATH"
  exit 1
fi
echo "bhc: $(bhc --version 2>/dev/null || echo unknown)"

# hx must detect BHC. Strip ANSI so the check does not depend on color state.
# Capture first: `hx doctor` exits non-zero whenever *any* check fails (e.g.
# no GHC on PATH), and under pipefail that would fail this BHC-only check.
doctor_out="$("$HX" doctor 2>&1 || true)"
if printf '%s\n' "$doctor_out" | sed -E 's/\x1b\[[0-9;]*m//g' | grep -qE 'bhc: [0-9]'; then
  ok "hx doctor detects BHC"
else
  bad "hx doctor does not detect BHC"
fi

# --- Scenario: a minimal multi-module BHC project builds and runs -----------
proj="$WORK/bhc-smoke"
mkdir -p "$proj/src" "$proj/app"
cat >"$proj/hx.toml" <<'TOML'
[project]
name = "bhc-smoke"
version = "0.1.0.0"

[compiler]
backend = "bhc"

[compiler.bhc]
profile = "default"

[build]
src_dirs = ["src", "app"]
TOML
cat >"$proj/src/Lib.hs" <<'HS'
module Lib (greeting, fib) where

greeting :: String
greeting = "hx+BHC pipeline ok"

fib :: Int -> Int
fib 0 = 0
fib 1 = 1
fib n = fib (n - 1) + fib (n - 2)
HS
cat >"$proj/app/Main.hs" <<'HS'
module Main where

import Lib (greeting, fib)

main :: IO ()
main = do
  putStrLn greeting
  putStrLn ("fib 10 = " ++ show (fib 10))
HS

echo "::group::bhc: build"
if ( cd "$proj" && "$HX" build ); then
  ok "hx build (BHC) succeeds"
else
  bad "hx build (BHC) failed"
fi
echo "::endgroup::"

# The produced executable runs and prints the expected output.
exe="$proj/.hx/bhc-build/bhc-smoke"
if [ -x "$exe" ] && out="$("$exe" 2>/dev/null)" && echo "$out" | grep -q "fib 10 = 55"; then
  ok "BHC-built executable runs (fib 10 = 55)"
else
  bad "BHC-built executable did not run as expected"
fi

# --- Scenario: hx reports BHC failures (BHC may exit 0 on errors) ----------
badp="$WORK/bhc-bad"
mkdir -p "$badp/src" "$badp/app"
cp "$proj/hx.toml" "$badp/hx.toml"
sed -i.bak 's/name = "bhc-smoke"/name = "bhc-bad"/' "$badp/hx.toml" && rm -f "$badp/hx.toml.bak"
cat >"$badp/src/Lib.hs" <<'HS'
module Lib (broken) where
-- References an out-of-scope name: a real compile error.
broken :: Int
broken = thisIsNotDefined 1
HS
cat >"$badp/app/Main.hs" <<'HS'
module Main where
import Lib (broken)
main :: IO ()
main = print broken
HS

echo "::group::bhc: failure is reported"
if ( cd "$badp" && "$HX" build ) >/dev/null 2>&1; then
  bad "hx build reported success on a broken BHC program (silent failure)"
else
  ok "hx build reports failure on a broken BHC program"
fi
echo "::endgroup::"

# --- Scenario: hx test builds and runs a cabal test-suite with BHC ----------
# BHC has no `test` subcommand; hx reads the `test-suite` stanza, compiles its
# `main-is` with BHC, and runs it. A passing suite exits 0; a failing one must
# make `hx test` exit non-zero (exit code 5) and name the suite.
tproj="$WORK/bhc-test"
mkdir -p "$tproj/src" "$tproj/test"
cat >"$tproj/hx.toml" <<'TOML'
[project]
name = "bhc-test"
version = "0.1.0.0"

[compiler]
backend = "bhc"

[build]
src_dirs = ["src"]
TOML
cat >"$tproj/bhc-test.cabal" <<'CABAL'
cabal-version: 2.4
name:          bhc-test
version:       0.1.0.0

library
  hs-source-dirs:  src
  exposed-modules: Double
  build-depends:   base

test-suite bhc-test-suite
  type:           exitcode-stdio-1.0
  main-is:        Main.hs
  hs-source-dirs: test
  build-depends:  base, bhc-test
CABAL
cat >"$tproj/src/Double.hs" <<'HS'
module Double (double) where

double :: Int -> Int
double x = x * 2
HS
write_test_main() {
  cat >"$tproj/test/Main.hs" <<HS
module Main where

import Double (double)
import System.Exit (exitFailure)

main :: IO ()
main =
  if double 21 == $1
    then putStrLn "bhc-test-suite: ok"
    else putStrLn "bhc-test-suite: unexpected result" >> exitFailure
HS
}

echo "::group::bhc: test (passing suite)"
write_test_main 42
if out="$(cd "$tproj" && "$HX" test 2>&1)"; then
  echo "$out"
  if echo "$out" | grep -q "bhc-test-suite: ok"; then
    ok "hx test (BHC) builds and runs a passing test suite"
  else
    bad "hx test (BHC) succeeded but the suite's output is missing"
  fi
else
  echo "$out"
  bad "hx test (BHC) failed on a passing test suite"
fi
echo "::endgroup::"

echo "::group::bhc: test (failing suite)"
write_test_main 43
out="$(cd "$tproj" && "$HX" test 2>&1)"
code=$?
echo "$out"
if [ "$code" -ne 0 ] && echo "$out" | sed -E 's/\x1b\[[0-9;]*m//g' | grep -q "failed: bhc-test-suite"; then
  ok "hx test (BHC) reports a failing test suite (exit $code)"
else
  bad "hx test (BHC) did not report a failing test suite (exit $code)"
fi
echo "::endgroup::"

# --- Summary ----------------------------------------------------------------
echo
echo "==================== bhc-smoke results ===================="
echo "  PASS=${PASS}  FAIL=${FAIL}"
echo "==========================================================="

[ "$FAIL" -eq 0 ]
