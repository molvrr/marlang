# Benchmarks

Benchmark `.mar` programs for measuring Marlang VM performance.

## Running

```bash
# Run individual benchmarks
cargo run -p marlang-cli -- run bench/fib.mar
cargo run -p marlang-cli -- run bench/fib-tail.mar
cargo run -p marlang-cli -- run bench/sum-loop.mar
cargo run -p marlang-cli -- run bench/pattern.mar
cargo run -p marlang-cli -- run bench/effects.mar
```

## Programs

| Benchmark | Description |
|-----------|-------------|
| `fib.mar` | Recursive fibonacci (naive, exponential) |
| `fib-tail.mar` | Tail-recursive fibonacci |
| `sum-loop.mar` | Sum 1 to N |
| `pattern.mar` | ADT pattern matching |
| `effects.mar` | Effect perform/resume cycles |

## Results: Rust VM

Measured on Apple Silicon, release build, 10 runs (median):

| Benchmark  | Time      |
|------------|-----------|
| fib        | 20.54ms   |
| fib-tail   | 14.76ms   |
| sum-loop   | 15.22ms   |
| pattern    | 30.10ms   |
| effects    | 14.60ms   |

Run with: `./bench/run.sh [RUNS]`
