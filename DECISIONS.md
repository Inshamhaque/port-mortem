# DECISIONS.md

Every non-trivial architectural divergence from the original (cJSON, C) with rationale.
This file is maintained incrementally as the port progresses.

---

## Context

- **Original:** [DaveGamble/cJSON](https://github.com/DaveGamble/cJSON) — C.
- **Port:** `cjson-rs` — Rust (edition 2024).
- **North star:** the original test suite under `tests/original/`, unmodified and hashed
  (`SHA256SUMS` + `.port-mortem.toml`), must pass against the port.

---

## Decisions made so far

### D1 — Target language: Rust, safe subset
- **Status:** decided
- **What:** Port cJSON to idiomatic Rust using only safe code (`#![forbid(unsafe_code)]`-style
  discipline; no `unsafe` blocks, no FFI to the C runtime).
- **Why:** The original is a single-header C library that leans heavily on raw pointers and
  manual memory management. Rust's ownership model gives the same zero-copy-ish parsing goals
  with memory safety guaranteed. This also targets the **Zero Unsafe** bonus.

### D2 — Library + CLI binary in one crate
- **Status:** partially scaffolded (`Cargo.toml` declares `lib` + `bin`; `lib.rs` present,
  `main.rs` pending)
- **What:** Two targets under one crate: a library (`cjson_rs`) exposing the API, and a binary
  (`cjson-rs`) exposing a CLI for differential testing against the original's CLI behavior.
- **Why:** The deliverables demand both a runnable artifact and an API surface that the original
  test suite can exercise. The binary is also the substrate for the differential fuzz harness.

### D3 — Idiomatic Rust error handling instead of cJSON's global error pointer
- **Status:** decided, `src/error.rs` implemented
- **What:** A `struct Error { offset, message }` implementing `Display` + `std::error::Error`.
  Replaces cJSON's `cJSON_GetErrorPtr()` global-error-state approach.
- **Why:** cJSON reports parse failures through a single global `const char *error_ptr`.
  That is not safe or idiomatic in Rust. Returning an error carrying the byte `offset` keeps the
  information the C tests assert on (parse failure position) without a global.

### D4 — Release profile tuned for honest benchmark reporting
- **Status:** decided (`Cargo.toml`)
- **What:** `codegen-units = 1`, `lto = true`, `strip = true` in the release profile.
- **Why:** The submission requires a benchmark report (p99, RSS, startup, throughput).
  This profile is chosen to represent the production artifact fairly in those numbers.

### D5 — Tests are the north star; originals stay untouched
- **Status:** decided
- **What:** `tests/original/` is a pinned, byte-for-byte copy of the cJSON test suite, hashed at
  kickoff (`suite_hash` in `.port-mortem.toml`). New port-specific tests live in `tests/port/`.
- **Why:** Test parity is the primary scoring criterion. Any future edit to an original test must
  be named here so judges can weigh it.

### D6 — `Value` is an owned `enum`, not a linked node
- **Status:** decided, `src/value.rs` implemented
- **What:** cJSON's `cJSON` struct uses `child`/`next`/`prev` pointers to build a doubly linked
  tree. The port models a document as a recursive `enum Value` (`Null`, `Bool`, `Number(f64)`,
  `String`, `Array(Vec<Value>)`, `Object(Vec<Member>)`, `Raw`) that owns its children.
- **Why:** Ownership by `Vec` gives the same tree with no manual lifetime or pointer juggling,
  and preserves cJSON's ordering guarantee. Object members stay in a `Vec<Member>` rather than a
  map because cJSON keeps insertion order and permits duplicate keys. `Raw` is kept for
  compatibility with callers that supply already-serialized JSON.

### D7 — Numbers stored as `f64`, matching cJSON
- **Status:** decided, `src/value.rs` implemented
- **What:** `Value::Number` holds an `f64`, exactly as cJSON stores all numbers in a `double`.
- **Why:** Behavioral parity with the original's printer and comparison (`cJSON_Print` and
  `cJSON_Compare` assume `double`) outweighs adding integer fidelity, which would silently change
  round-trip output and break test parity.

### D8 — Parser is text-in, `Value`-out, and owns the whole document
- **Status:** decided, `src/parser.rs` implemented (17 unit tests green)
- **What:** `parse(&str) -> Result<Value, Error>` runs a recursive-descent parse over the bytes and
  enforces `NESTING_LIMIT`. On a trailing-character error it returns an [`Error`] carrying the byte
  offset rather than cJSON's global error pointer.
- **Why:** Recursive descent maps directly onto cJSON's parser structure, and building an owned
  `Value` up front means no tree is mutated mid-parse. Error offsets keep the position information
  the C tests assert on.

### D9 — Minify works on text, not the parsed tree
- **Status:** decided, `src/minify.rs` implemented
- **What:** `minify(&str) -> String` strips whitespace and comments directly from source text, as
  `cJSON_Minify` does, instead of round-tripping through a `Value`.
- **Why:** cJSON's minifier is a textual filter; mirroring it keeps behavior identical, including
  the C implementation's `//` and `/* ... */` comment-removal extension and its lone-slash handling.

---

## Open / to be decided

- How the C test harness (Unity `TEST_ASSERT_*`) maps onto Rust `#[test]` without editing the
  original `.c` files (likely a translation layer under `tests/port/`).
- Printer (`print`/`print_unformatted`) — cJSON's `cJSON_Print`/`cJSON_PrintUnformatted`.
- Mutation, JSON Pointer/Patch, and the `utils` helpers — described in the `lib.rs` docs but not
  yet built.
- `src/main.rs` for the CLI binary declared in `Cargo.toml`.
