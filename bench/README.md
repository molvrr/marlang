# Benchmark Results

## Tree-Walker vs Bytecode VM

| Benchmark | Tree-Walker | VM       | Speedup | Match |
|-----------|-------------|----------|---------|-------|
| fib       | 915.32ms    | 310.50ms | 2.95x   | OK    |
| fib-tail  | 83µs        | 23µs     | 3.62x   | OK    |
| sum-loop  | 53.52ms     | 11.04ms  | 4.85x   | OK    |
| pattern   | 150.72ms    | 13.85ms  | 10.88x  | OK    |
| effects   | 121.08ms    | 1.68ms   | 72.20x  | OK    |

Config: warmup 2 runs, measured 5 runs (median).
