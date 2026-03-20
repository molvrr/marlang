#!/usr/bin/env bash
# Benchmark runner for Marlang VM
# Usage: ./bench/run.sh [RUNS]
#   RUNS: number of measured runs (default: 5)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PORT_DIR="$REPO_ROOT/port"
RUNS="${1:-5}"

BENCHMARKS=(
  "fib:$SCRIPT_DIR/fib.mar"
  "fib-tail:$SCRIPT_DIR/fib-tail.mar"
  "sum-loop:$SCRIPT_DIR/sum-loop.mar"
  "pattern:$SCRIPT_DIR/pattern.mar"
  "effects:$SCRIPT_DIR/effects.mar"
  "json:$SCRIPT_DIR/json.mar"
)

# Build release binary
echo "Building release binary..."
cargo build --release -p marlang-cli --manifest-path "$PORT_DIR/Cargo.toml" 2>&1 | tail -1
MAR="$PORT_DIR/target/release/mar"
echo ""

# Get time in ms using perl (avoids spawning python per call)
now_ms() {
  perl -MTime::HiRes=time -e 'printf "%.4f\n", time * 1000'
}

# Median of an array (sorts and picks middle)
median() {
  printf '%s\n' "$@" | sort -n | awk -v n="$#" 'NR==int(n/2)+1{print}'
}

printf "%-12s %10s  (%d runs, median)\n" "Benchmark" "Time" "$RUNS"
printf "%-12s %10s\n" "-----------" "----------"

for entry in "${BENCHMARKS[@]}"; do
  name="${entry%%:*}"
  file="${entry#*:}"

  times=()
  for ((i = 1; i <= RUNS; i++)); do
    start=$(now_ms)
    "$MAR" run "$file" > /dev/null 2>&1
    end=$(now_ms)
    elapsed=$(perl -e "printf '%.2f', $end - $start")
    times+=("$elapsed")
  done

  med=$(median "${times[@]}")
  printf "%-12s %8sms\n" "$name" "$med"
done
