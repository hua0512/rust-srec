# HLS processing benchmarks

Run from the repository root:

```sh
cargo bench --locked -p hls-fix --bench processing
```

The default input is the small, checked-in H.264 TS fixture. For representative
segment sizes, set `HLS_BENCH_TS` to a local TS recording before running the same
command. For example, in PowerShell:

```powershell
$env:HLS_BENCH_TS = 'D:\recordings\segment.ts'
cargo bench --locked -p hls-fix --bench processing -- --save-baseline before
# After changing the implementation, use the same input and build profile:
cargo bench --locked -p hls-fix --bench processing -- --baseline before
```

`analysis_base` and `analysis_resolution` create a fresh segment each iteration
to measure parsing rather than a cache hit. `split_cold` includes parsing and
structural comparison; `split_cached` measures comparison with an existing
analysis and reports operations per second, not media throughput. The fMP4
case measures an init plus four media items crossing the gathering threshold.

These are CPU microbenchmarks without network traffic, file writes, or tracing
subscribers. They do not predict total recording throughput. Compare release
builds on the same machine with the same input; timings are not CI assertions.
