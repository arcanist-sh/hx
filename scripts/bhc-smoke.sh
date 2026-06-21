#!/usr/bin/env bash
#
# BHC pipeline smoke test for hx.
#
# Proves the hx -> BHC build pipeline end to end against a real BHC toolchain:
# detection, the correct CLI invocation, LIBRARY_PATH wiring for the runtime
# libraries, linking, and honest failure reporting (BHC exits 0 even on errors).
#
# It deliberately uses a *minimal* program that BHC 0.2.3 can actually compile.
# The `numeric` and `server` templates are NOT exercised here: BHC 0.2.3 cannot
# yet compile polymorphic numeric code (`sum` over `Double`, `fromIntegral`) or
# provide Servant, so those remain gated until the compiler gains those
# features (tracked in the BHC issue tracker).
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
if "$HX" doctor 2>&1 | sed -E 's/\x1b\[[0-9;]*m//g' | grep -qE 'bhc: [0-9]'; then
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

# --- Scenario: hx reports BHC failures (BHC itself exits 0 on errors) -------
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

# --- Summary ----------------------------------------------------------------
echo
echo "==================== bhc-smoke results ===================="
echo "  PASS=${PASS}  FAIL=${FAIL}"
echo "==========================================================="

[ "$FAIL" -eq 0 ]
