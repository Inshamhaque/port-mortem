# cjson-rs — a safe Rust port of cJSON v1.7.19

A from-scratch, memory-safe port of [cJSON](https://github.com/DaveGamble/cJSON)
(v1.7.19) to Rust (edition 2024). The port keeps cJSON's exact observable
behavior — its parser permissiveness, its quirky printer, its JSON Pointer /
Patch / merge-patch utilities — while replacing raw pointers and manual memory
management with ownership, checked bounds, and a `Result`-based API.

**The original test suite passes, unmodified, against the port.** The C tests
under `tests/original/` (hashed at kickoff, byte-for-byte untouched) compile and
run against the Rust code through a C-ABI compatibility layer: **21/21 binaries
pass**, and a differential fuzzer comparing the two independent Rust parsers
runs 60s+ with **zero divergences**.

## Why port to Rust?

| | cJSON (C) | cjson-rs (Rust) |
|---|---|---|
| Memory safety | raw pointers, manual `free` | guaranteed by the type system |
| Errors | global `cJSON_GetErrorPtr` | `Result<Value, Error>` with offset |
| Mutations | silent pointer surgery | checked operations, `Result<_, _>` |
| Parsing | `strtod` permissiveness | same behavior, length-based (no NUL bugs) |
| Threading | global allocator hooks | per-value, no globals |

Every non-trivial divergence from the original is logged with its rationale in
[DECISIONS.md](DECISIONS.md).

## Repository layout

```
.
├── README.md            this file
├── DECISIONS.md         every non-trivial divergence + why
├── Dockerfile           one command to a runnable artifact (and proof of parity)
├── .port-mortem.toml    track A, source URL, kickoff hashes
├── Makefile             build/test/fuzz/bench harness
├── src/                 the Rust port (idiomatic, safe; ffi.rs is the single
│                        documented unsafe zone implementing the C ABI)
├── tests/
│   ├── original/        the cJSON test suite, unmodified (SHA-256 pinned)
│   ├── cJSON.h/.c       test-build shim: declares the C surface the tests use
│   ├── cJSON_Utils.h    shim for the cJSON_Utils layer
│   └── port/            Rust-native integration tests mirroring the original C suite
├── fuzz/
│   ├── driver.c         C oracle over the FFI layer (cJSON_Parse + cJSON_Print)
│   ├── harness.py       differential fuzzer (FFI vs safe core)
│   └── log.txt          60s+ run, zero divergences
└── bench/
    ├── bench.rs         in-process throughput + latency binary
    ├── run.py           assembles results.json (adds startup + RSS)
    ├── methodology.md   how each metric was measured
    └── results.json     p99, RSS, startup, throughput
```

## Build

Requires a Rust toolchain (edition 2024, tested with 1.97) and a C compiler.

```sh
make build            # cargo build --release: libcjson_rs.a + cjson-rs CLI
```

## Test

```sh
make test             # cargo test: 40 lib + 5 CLI + 41 tests/port assertions
make test-original    # compile+run the original C suite against the FFI (21/21)
make verify           # both of the above
```

`make test-original` compiles each original test with Unity, links
`libcjson_rs.a` (the Rust FFI layer), and runs it from a staging dir with the
`inputs/` and `json-patch-tests/` fixtures, replicating the original CMake
layout. Verify the suite is untouched:

```sh
find tests/original -type f -print0 | sort -z | xargs -0 cat | shasum -a256
#  e12e9a6c5ae59e313c9587367574a9412eaed55db7f38690b68f667673b032f5
```

## CLI

```sh
cjson-rs parse doc.json            # validate + print (tab-formatted)
cjson-rs print doc.json            # compact
cjson-rs print --format doc.json   # tab-formatted
cjson-rs minify - < doc.json       # strip whitespace/comments
cjson-rs get doc.json /a/b/0       # JSON Pointer lookup
cjson-rs patch doc.json patch.json # apply a JSON Patch
```

## Differential fuzzing

`fuzz/harness.py` feeds the same PRNG-driven inputs (structured JSON, corpus
mutations, raw bytes) to two independent implementations — the FFI oracle
(`fuzz/driver.c` → `cJSON_ParseWithOpts`/`cJSON_PrintUnformatted`) and the safe
core (`cjson-rs print -` → `parse()`/`print_unformatted()`) — and diffs
parse-status and printed bytes.

```sh
make fuzz             # 60s run, writes fuzz/log.txt
python3 fuzz/harness.py --driver build/fuzz-driver --cli target/release/cjson-rs \
    --duration 60 --seed 7 --log fuzz/log.txt
```

The committed `fuzz/log.txt` records a 60s+ run with **0 divergences**. The
fuzzer is how the parser was aligned with cJSON's permissiveness (see
DECISIONS.md D16).

## Benchmark

```sh
make bench            # writes bench/results.json
```

See [bench/methodology.md](bench/methodology.md) for how each figure is
measured and its caveats.

## Docker

The Dockerfile builds the release binary, then **runs the unmodified original C
suite inside the build** — so `docker build` itself proves parity and fails if
the port regresses:

```sh
docker build -t cjson-rs .
echo '{"hello":"world"}' | docker run --rm -i cjson-rs parse -
```

## Port-mortem metadata

- **Track:** A
- **Source:** https://github.com/DaveGamble/cJSON (v1.7.19)
- **Kickoff:** 2026-07-31T20:32:55Z
- **Suite hash:** `e12e9a6c...732b032f5` (see [.port-mortem.toml](.port-mortem.toml))

## License

MIT (matching cJSON, whose header is reproduced in `tests/cJSON.h` /
`tests/cJSON_Utils.h` for the shim).
