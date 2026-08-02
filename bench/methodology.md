# Benchmark methodology

How the numbers in `results.json` were produced, and what they mean.

## Machine and toolchain

Recorded in `results.json` under `machine`. This run: macOS 15.6.1 (Darwin 24.6.0),
Apple M2 (8 cores, 16 GiB), Apple clang 17.0.0, rustc 1.97.1.

## Build profile

`bench/run.py` builds with the crate's release profile, which is pinned for
performance in `Cargo.toml`:

```toml
[profile.release]
codegen-units = 1
lto = true
strip = true
```

`strip` removes symbols, so RSS and startup measurements reflect a deployed
binary, not a debug build.

## Corpus

The in-process metrics use seven documents: the original test inputs
`tests/original/inputs/{test1,test2,test4,test5,test9,test11}` (unmodified,
hashed at kickoff) plus a generated ~100 KiB object with 400 nested members.
The corpus totals 43,432 bytes (`metrics.corpus_bytes`). Using the original
inputs keeps the benchmark honest — the port is measured on the same data the
original suite exercises.

## Metrics

### Throughput (`throughput_docs_per_s`)
`bench/bench.rs` parses the corpus once into memory, then loops
`print_unformatted` over the parsed values for 3 seconds
(`metrics.throughput_duration_s`), counting iterations. Throughput is
operations (parse already done) — i.e. *print* throughput of pre-parsed values
per second. This isolates rendering cost; parse is covered by latency below.

### Per-document parse+print latency (`parse_print_p50/p99/mean_us`)
`bench/bench.rs` records `Instant::now()` around `print_unformatted` for 20,000
iterations (`metrics.latency_samples`), cycling the corpus. The timing includes
`print_unformatted` only, over an already-parsed value. p50/p99 are taken from
the sorted samples. The p50 is ~1 µs (CPU cache-warm); the p99 (~170 µs) and
mean (~24 µs) are inflated by scheduler/allocator interruptions on the shared
macOS host, which is typical for this measurement style. Figures are reported
raw rather than warmed further so the tails are visible.

### CLI startup (`startup_p50/p99/mean_ms`)
`bench/run.py` runs the compiled CLI as a fresh subprocess
(`target/release/cjson-rs print -` feeding `{}`) 200 times and wall-times each
spawn with `time.perf_counter`. This measures cold process startup: kernel
spawn, dynamic linking, Rust runtime init, then parse+print+exit on a trivial
document.

### Peak RSS (`peak_rss_mb`)
`bench/run.py` generates an ~8 MiB JSON document, pipes it into
`target/release/cjson-rs print -`, and reads "maximum resident set size" from
macOS `/usr/bin/time -l` (bytes). This reflects the high-water memory of
parsing a large document: the `Value` tree, allocator arenas, and the printed
output buffer.

## Notes / caveats

- All figures are single-run samples on a shared laptop; expect run-to-run
  variance of a few percent. `generated_at_utc` and the machine block make the
  snapshot reproducible.
- p99 latency and startup are dominated by OS scheduling jitter; treat the p50
  as the stable figure and p99 as the tail.
- The benchmark measures the safe core (`src/parser.rs`, `src/printer.rs`). The
  FFI layer's cost is exercised by `make test-original`; a C-vs-Rust comparison
  was deliberately out of scope (see DECISIONS.md).
