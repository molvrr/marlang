#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
PORT_DIR="$REPO_ROOT/port"

# Build release binary
cargo build --release -p marlang-cli --manifest-path "$PORT_DIR/Cargo.toml" 2>&1 | tail -1
MAR="$PORT_DIR/target/release/mar"

# Run fib40 benchmark (5 runs, take median)
RUNS=5
times=()
for ((i = 1; i <= RUNS; i++)); do
    start=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
    "$MAR" run bench/fib40.mar > /dev/null 2>&1
    end=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
    elapsed=$(perl -e "printf '%.3f', $end - $start")
    times+=("$elapsed")
done
med_mar=$(printf '%s\n' "${times[@]}" | sort -n | awk -v n="${#times[@]}" 'NR==int(n/2)+1{print}')

# Run bun reference (5 runs, take median)
bun_times=()
for ((i = 1; i <= RUNS; i++)); do
    start=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
    bun bench/fib40.ts > /dev/null 2>&1
    end=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
    elapsed=$(perl -e "printf '%.3f', $end - $start")
    bun_times+=("$elapsed")
done
med_bun=$(printf '%s\n' "${bun_times[@]}" | sort -n | awk -v n="${#bun_times[@]}" 'NR==int(n/2)+1{print}')

echo "METRIC fib40_s=$med_mar"
echo "METRIC bun_s=$med_bun"
echo "Ratio: $(perl -e "printf '%.1fx', $med_mar / $med_bun")"
