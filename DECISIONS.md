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
- **Status:** scaffolded (`Cargo.toml` declares `lib` + `bin`; `lib.rs`/`main.rs` pending)
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

---

## Open / to be decided

- Concrete AST / `Value` representation in Rust (cJSON's `cJSON` struct with `child`/`next`
  sibling pointers maps to a `Vec`-based tree in idiomatic Rust).
- Number model: cJSON stores numbers as `double`; decide whether the port preserves that
  exactly (for behavioral parity on `print_number`) or adds integer fidelity.
- How the C test harness (Unity `TEST_ASSERT_*`) maps onto Rust `#[test]` without editing the
  original `.c` files (likely a translation layer under `tests/port/`).
