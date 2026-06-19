#!/bin/bash
# =============================================================================
# hx Benchmark Comparison Script
# =============================================================================
#
# This script runs comprehensive benchmarks comparing hx to cabal and stack.
# Results are output in a format suitable for documentation.
#
# Usage:
#   ./scripts/benchmark-comparison.sh [--json] [--iterations N]
#
# Requirements:
#   - hx installed and in PATH
#   - cabal installed and in PATH (optional, for comparison)
#   - stack installed and in PATH (optional, for comparison)
#   - hyperfine (for accurate benchmarking): cargo install hyperfine
#   - jq (for JSON processing)
# =============================================================================

set -e

# Configuration
ITERATIONS=${ITERATIONS:-5}
WARMUP=${WARMUP:-2}
OUTPUT_FORMAT="markdown"
RESULTS_DIR="benchmark-results"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --json)
            OUTPUT_FORMAT="json"
            shift
            ;;
        --iterations)
            ITERATIONS="$2"
            shift 2
            ;;
        --output)
            RESULTS_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  hx Benchmark Comparison Suite${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Create results directory
mkdir -p "$RESULTS_DIR"

# Detect available tools
HAS_HX=$(command -v hx &> /dev/null && echo "yes" || echo "no")
HAS_CABAL=$(command -v cabal &> /dev/null && echo "yes" || echo "no")
HAS_STACK=$(command -v stack &> /dev/null && echo "yes" || echo "no")
HAS_HYPERFINE=$(command -v hyperfine &> /dev/null && echo "yes" || echo "no")

echo "Tool Detection:"
echo "  hx:        $HAS_HX $(hx --version 2>/dev/null | head -1 || echo '')"
echo "  cabal:     $HAS_CABAL $(cabal --version 2>/dev/null | head -1 || echo '')"
echo "  stack:     $HAS_STACK $(stack --version 2>/dev/null | head -1 || echo '')"
echo "  hyperfine: $HAS_HYPERFINE"
echo ""

if [[ "$HAS_HX" != "yes" ]]; then
    echo -e "${RED}Error: hx not found in PATH${NC}"
    exit 1
fi

# Create temporary directory for test projects
BENCH_DIR=$(mktemp -d)
trap "rm -rf $BENCH_DIR" EXIT

echo "Benchmark directory: $BENCH_DIR"
echo "Iterations: $ITERATIONS"
echo "Warmup: $WARMUP"
echo ""

# =============================================================================
# Benchmark Functions
# =============================================================================

run_timed() {
    local label="$1"
    local cmd="$2"
    local start end elapsed

    start=$(date +%s.%N)
    eval "$cmd" > /dev/null 2>&1
    end=$(date +%s.%N)
    elapsed=$(echo "$end - $start" | bc)

    printf "  %-30s %8.3fs\n" "$label" "$elapsed"
    echo "$elapsed"
}

benchmark_with_hyperfine() {
    local name="$1"
    shift
    local commands=("$@")

    if [[ "$HAS_HYPERFINE" == "yes" ]]; then
        echo -e "${YELLOW}Running: $name${NC}"
        hyperfine --warmup "$WARMUP" --runs "$ITERATIONS" \
            --export-json "$RESULTS_DIR/${name}.json" \
            --export-markdown "$RESULTS_DIR/${name}.md" \
            "${commands[@]}"
        echo ""
    else
        echo -e "${YELLOW}Running: $name (no hyperfine, using time)${NC}"
        for cmd in "${commands[@]}"; do
            echo "  Command: $cmd"
            for i in $(seq 1 $ITERATIONS); do
                time eval "$cmd" > /dev/null 2>&1
            done
        done
        echo ""
    fi
}

# =============================================================================
# Benchmark 1: CLI Startup Time (--help)
# =============================================================================

echo -e "${GREEN}=== Benchmark 1: CLI Startup Time ===${NC}"
echo ""

STARTUP_CMDS=("hx --help")
[[ "$HAS_CABAL" == "yes" ]] && STARTUP_CMDS+=("cabal --help")
[[ "$HAS_STACK" == "yes" ]] && STARTUP_CMDS+=("stack --help")

benchmark_with_hyperfine "startup" "${STARTUP_CMDS[@]}"

# =============================================================================
# Benchmark 2: Version Check
# =============================================================================

echo -e "${GREEN}=== Benchmark 2: Version Check ===${NC}"
echo ""

VERSION_CMDS=("hx --version")
[[ "$HAS_CABAL" == "yes" ]] && VERSION_CMDS+=("cabal --version")
[[ "$HAS_STACK" == "yes" ]] && VERSION_CMDS+=("stack --version")

benchmark_with_hyperfine "version" "${VERSION_CMDS[@]}"

# =============================================================================
# Benchmark 3: Project Initialization
# =============================================================================

echo -e "${GREEN}=== Benchmark 3: Project Initialization ===${NC}"
echo ""

# hx init
HX_INIT_DIR="$BENCH_DIR/hx-init-test"
INIT_CMDS=("rm -rf $HX_INIT_DIR && hx init --name hx-init-test $HX_INIT_DIR")

# cabal init
if [[ "$HAS_CABAL" == "yes" ]]; then
    CABAL_INIT_DIR="$BENCH_DIR/cabal-init-test"
    INIT_CMDS+=("rm -rf $CABAL_INIT_DIR && mkdir -p $CABAL_INIT_DIR && cd $CABAL_INIT_DIR && cabal init --non-interactive --package-name=cabal-init-test --no-comments")
fi

# stack new
if [[ "$HAS_STACK" == "yes" ]]; then
    STACK_INIT_DIR="$BENCH_DIR/stack-init-test"
    INIT_CMDS+=("rm -rf $STACK_INIT_DIR && cd $BENCH_DIR && stack new stack-init-test simple --resolver=lts-22.7")
fi

benchmark_with_hyperfine "init" "${INIT_CMDS[@]}"

# =============================================================================
# Benchmark 4: Build (Simple Project - Native Mode)
# =============================================================================

echo -e "${GREEN}=== Benchmark 4: Build Performance (Simple Project) ===${NC}"
echo ""

# Create a simple test project
SIMPLE_PROJECT="$BENCH_DIR/simple-project"
hx init --name simple-project "$SIMPLE_PROJECT"

# Create a simple Main.hs
cat > "$SIMPLE_PROJECT/src/Main.hs" << 'EOF'
module Main where

import Data.List (sort)

main :: IO ()
main = do
    let numbers = [5, 3, 8, 1, 9, 2, 7, 4, 6]
    print (sort numbers)
    putStrLn "Hello from hx benchmark!"
EOF

echo "Project created at: $SIMPLE_PROJECT"
echo ""

# Cold build comparison
echo "Cold Build (after clean):"
BUILD_CMDS=("cd $SIMPLE_PROJECT && rm -rf .hx dist-newstyle && hx build --native")
[[ "$HAS_CABAL" == "yes" ]] && BUILD_CMDS+=("cd $SIMPLE_PROJECT && rm -rf .hx dist-newstyle && cabal build")

benchmark_with_hyperfine "build-cold" "${BUILD_CMDS[@]}"

# Incremental build (no changes)
echo "Incremental Build (no changes):"
# First, do a build to warm up
cd "$SIMPLE_PROJECT" && hx build --native > /dev/null 2>&1

INCR_CMDS=("cd $SIMPLE_PROJECT && hx build --native")
[[ "$HAS_CABAL" == "yes" ]] && INCR_CMDS+=("cd $SIMPLE_PROJECT && cabal build")

benchmark_with_hyperfine "build-incremental" "${INCR_CMDS[@]}"

# =============================================================================
# Benchmark 5: Clean Operation
# =============================================================================

echo -e "${GREEN}=== Benchmark 5: Clean Operation ===${NC}"
echo ""

# Create artifacts to clean
mkdir -p "$SIMPLE_PROJECT/.hx"
mkdir -p "$SIMPLE_PROJECT/dist-newstyle"
for i in {1..50}; do
    echo "fake" > "$SIMPLE_PROJECT/.hx/artifact-$i.o"
    mkdir -p "$SIMPLE_PROJECT/dist-newstyle/build-$i"
done

CLEAN_CMDS=()

# hx clean
CLEAN_CMDS+=("cd $SIMPLE_PROJECT && mkdir -p .hx dist-newstyle && hx clean")

# cabal clean
if [[ "$HAS_CABAL" == "yes" ]]; then
    CLEAN_CMDS+=("cd $SIMPLE_PROJECT && mkdir -p dist-newstyle && cabal clean")
fi

benchmark_with_hyperfine "clean" "${CLEAN_CMDS[@]}"

# =============================================================================
# Benchmark 6: Doctor/Diagnostics
# =============================================================================

echo -e "${GREEN}=== Benchmark 6: Environment Diagnostics ===${NC}"
echo ""

DOCTOR_CMDS=("hx doctor")
# Note: cabal and stack don't have direct equivalents

benchmark_with_hyperfine "doctor" "${DOCTOR_CMDS[@]}"

# =============================================================================
# Benchmark 7: Shell Completions Generation
# =============================================================================

echo -e "${GREEN}=== Benchmark 7: Shell Completions ===${NC}"
echo ""

COMP_CMDS=("hx completions bash")
COMP_CMDS+=("hx completions zsh")
COMP_CMDS+=("hx completions fish")

benchmark_with_hyperfine "completions" "${COMP_CMDS[@]}"

# =============================================================================
# Generate Summary Report
# =============================================================================

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  Benchmark Summary${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

if [[ "$HAS_HYPERFINE" == "yes" ]]; then
    echo "Detailed results saved to: $RESULTS_DIR/"
    echo ""

    # Generate combined markdown report
    SUMMARY_FILE="$RESULTS_DIR/BENCHMARK_SUMMARY.md"

    cat > "$SUMMARY_FILE" << EOF
# hx Benchmark Results

Generated: $(date -u +"%Y-%m-%d %H:%M:%S UTC")

## Environment

| Property | Value |
|----------|-------|
| hx version | $(hx --version | head -1) |
| Platform | $(uname -s) $(uname -m) |
| Date | $(date +"%Y-%m-%d") |

## Results

EOF

    # Append each benchmark result
    for md_file in "$RESULTS_DIR"/*.md; do
        if [[ -f "$md_file" && "$md_file" != "$SUMMARY_FILE" ]]; then
            echo "### $(basename "$md_file" .md | tr '-' ' ' | tr '[:lower:]' '[:upper:]')" >> "$SUMMARY_FILE"
            echo "" >> "$SUMMARY_FILE"
            cat "$md_file" >> "$SUMMARY_FILE"
            echo "" >> "$SUMMARY_FILE"
        fi
    done

    echo "Summary report: $SUMMARY_FILE"
fi

echo ""
echo -e "${GREEN}Benchmarks complete!${NC}"
