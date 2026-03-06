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

Measured on Apple Silicon, release build, 5 runs (median):

| Benchmark  | Time      |
|------------|-----------|
| fib        | 190.83ms  |
| fib-tail   | 20.35ms   |
| sum-loop   | 25.63ms   |
| pattern    | 26.93ms   |
| effects    | 21.28ms   |

Run with: `./bench/run.sh [RUNS]`

## Historical: TypeScript Tree-Walker vs TS VM

These results are from the archived TypeScript implementation (`ts-archive/bench/run.ts`):

| Benchmark | Tree-Walker | TS VM    | Speedup |
|-----------|-------------|----------|---------|
| fib       | 915.32ms    | 310.50ms | 2.95x   |
| fib-tail  | 83us        | 23us     | 3.62x   |
| sum-loop  | 53.52ms     | 11.04ms  | 4.85x   |
| pattern   | 150.72ms    | 13.85ms  | 10.88x  |
| effects   | 121.08ms    | 1.68ms   | 72.20x  |

Config: warmup 2 runs, measured 5 runs (median).
