#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
PORT_DIR="$REPO_ROOT/port"

# Build release binary
cargo build --release -p marlang-cli --manifest-path "$PORT_DIR/Cargo.toml" 2>&1 | tail -1
MAR="$PORT_DIR/target/release/mar"

RUNS=5

# Helper: run N iterations, return median time in ms
bench() {
    local file="$1"
    local times=()
    for ((i = 1; i <= RUNS; i++)); do
        start=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
        "$MAR" run "$file" > /dev/null 2>&1
        end=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
        elapsed=$(perl -e "printf '%.2f', ($end - $start) * 1000")
        times+=("$elapsed")
    done
    printf '%s\n' "${times[@]}" | sort -n | awk -v n="${#times[@]}" 'NR==int(n/2)+1{print}'
}

# Measure each benchmark
effects_ms=$(bench "bench/effects.mar")
json_ms=$(bench "bench/json.mar")
fib_ms=$(bench "bench/fib.mar")
pattern_ms=$(bench "bench/pattern.mar")

# Primary metric: sum of effect-heavy benchmarks (effects + json)
total=$(perl -e "printf '%.2f', $effects_ms + $json_ms")

echo "METRIC total_ms=$total"
echo "METRIC effects_ms=$effects_ms"
echo "METRIC json_ms=$json_ms"
echo "METRIC fib_ms=$fib_ms"
echo "METRIC pattern_ms=$pattern_ms"
