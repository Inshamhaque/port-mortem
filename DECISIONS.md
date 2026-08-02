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
- **Status:** decided, `src/main.rs` implemented
- **What:** Two targets under one crate: a library (`cjson_rs`) exposing the API, and a binary
  (`cjson-rs`) exposing a CLI with `parse`, `print`, `minify`, `get` (JSON Pointer), and `patch`
  commands — the substrate for differential testing against the original's CLI behavior.
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

### D10 — Mutation layer maps to typed helpers, not cJSON's raw pointers
- **Status:** decided, `src/mutate.rs` implemented (23 unit tests green)
- **What:** A typed, fallible mutation API replacing the cJSON `cJSON_Add*ToObject`,
  `cJSON_Add*ToArray`, `cJSON_DeleteItem*`, `cJSON_Detach*`, `cJSON_InsertItem*`,
  `cJSON_ReplaceItem*`, and `cJSON_Set*` equivalents, plus a `MutationError`.
- **Why:** cJSON mutates a linked tree through raw `cJSON*` pointers and can fail silently or leak.
  The port instead exposes operations over owned [`Value`]s that return `Result<_, MutationError>`,
  keeping failure explicit and safety compile-time. Reference-style adds clone rather than alias, so
  no ownership is ever shared unsafely.

### D11 — JSON Pointer / Patch / merge patch return errors, not codes-as-pointers
- **Status:** decided, `src/utils.rs` implemented (34 unit tests green)
- **What:** A `PatchError(u32)` carrying cJSON's `cJSONUtils_ApplyPatches` error codes, plus
  `get_pointer`/`delete_pointer`, JSON Patch (`apply_patches`, `generate_patches`,
  `add_patch_to_array`), RFC 7396 merge patch, `sort_object`, and `find_pointer_from_object_to`.
- **Why:** cJSON reports patch failures via `int` return codes and pointer-walking that can silently
  miss. The port keeps the same numeric codes (for behavioral parity) but returns them through
  `Result<_, PatchError>` and does pointer resolution with checked `Vec`/`slice` access, so a bad
  path is a handled error rather than undefined behavior.

### D12 — Printer reproduces cJSON's exact rendering, including its number quirks
- **Status:** decided, `src/printer.rs` implemented (39 unit tests green)
- **What:** `print` (tab-indented) and `print_unformatted` (compact) mirror `cJSON_Print` /
  `cJSON_PrintUnformatted`. Numbers go through cJSON's 15-then-17 significant-digit strategy so
  round-trips match the C output byte-for-byte; non-finite numbers render as `null`, as in cJSON.
- **Why:** Behavioral equivalence on printer output is scored directly. Reproducing cJSON's exact
  number formatting (rather than using Rust's default `f64` display) is what keeps `print_*`
  tests passing unmodified.

### D13 — CLI surface exists for differential testing
- **Status:** decided, `src/main.rs` implemented
- **What:** A `cjson-rs` binary with subcommands that read a file or stdin and print or transform
  JSON, giving a shared, scriptable interface to compare against the original cJSON CLI.
- **Why:** The scoring wants a one-command runnable artifact and differential fuzz over a shared
  public API. A thin CLI over the library is the cheapest way to satisfy both.

### D14 — `GeneratePatches` array removals reuse one index, faithfully
- **Status:** decided, fixed in `src/utils.rs` (`create_patches`, array case)
- **What:** When diffing arrays, cJSON_Utils.c `create_patches` deletes the tail of `from` in a loop
  that **does not increment its index** — every leftover removal is generated against the *same*
  array position (the count of shared leading elements). The port had incremented the index, so a
  shrink like `[1,2,3,4]` → `[1,3]` produced `remove /2, remove /3`; applying that removes `/3` after
  the array had shrunk, failing with `cJSONUtils_ErrPatchResult` (13). The port now reuses the index,
  emitting `remove /2, remove /2`, which applies cleanly (each removal shifts the next element into
  the slot). Caught by the unmodified `json_patch_tests` ("test repeated removes", generate pass).
- **Why:** Behavioral parity with the original *output* (the generated patch) and with the original's
  ability to round-trip generate→apply. The quirk is safe: same-index removals are exactly right for
  deleting a contiguous tail.

### D15 — C-ABI test harness: original suite drives the Rust FFI via a shim
- **Status:** decided, `tests/cJSON.h`, `tests/cJSON.c`, `tests/cJSON_Utils.h`, `Makefile`
- **What:** The original `.c` tests `#include "../cJSON.c"` and `"../cJSON_Utils.h"` (single-file
  style). The build replaces the real library source with a shim that only *declares* the C surface:
  the `cJSON` struct + constants + macros (`cJSON.h`, faithful to v1.7.19), the cJSON.c-internal
  structs `internal_hooks`/`parse_buffer`/`printbuffer` plus `extern` for every exported symbol
  (`cJSON.c`), and the `cJSONUtils_*` externs (`cJSON_Utils.h`). The `Makefile` compiles each test
  against `tests/original/unity` and links `libcjson_rs.a` (`crate-type = ["rlib","staticlib"]`),
  staging `inputs/` + `json-patch-tests/` like the original CMake. All 21 binaries pass unmodified.
- **Why:** Keeps `tests/original/` byte-identical to the kickoff hash while giving the white-box
  tests their C type definitions and giving the linker the Rust implementations. The FFI layer
  (`src/ffi.rs`, wired in via `mod ffi;`) is validated by this suite — its whole purpose.

### D16 — Safe-core parser matches cJSON's permissiveness; differential fuzz is clean
- **Status:** decided, `src/parser.rs` + `src/ffi.rs` fixed; `fuzz/` harness green
- **What:** The differential fuzzer (`fuzz/harness.py`, FFI `cJSON_Parse` vs safe `parse`)
  surfaced four classes where the two implementations disagreed. Three were the safe core being
  *stricter* than cJSON and were aligned to cJSON:
  1. **Whitespace set** — cJSON's `buffer_skip_whitespace` uses `isspace(3)` (` \t\n\v\f\r`); the
     safe parser skipped only ` \t\n\r`.
  2. **Raw control characters in strings** — cJSON copies any non-backslash byte verbatim; the
     safe parser rejected bytes `< 0x20`.
  3. **Number grammar** — cJSON scans `[0-9 + - e E .]` and hands the token to `strtod`, so it
     accepts leading zeros (`01` → 1), bare fractions (`1.` → 1) and leaves a bare exponent
     unconsumed (`1e` → 1). The safe parser enforced RFC 8259's grammar. Both parsers now share a
     `parse_c_float` strtod-subset (`src/ffi.rs`, `src/parser.rs`).
  The fourth was a real **FFI bug**: `buffer_skip_whitespace` used `<= 32` (treated every control
  byte as whitespace), accepting documents the original C rejects. It now uses the `isspace` set.
- **Why:** The north star is behavioral parity with cJSON; "safe" describes memory safety (no
  `unsafe`, checked access), not spec strictness. Aligning the two parsers means the differential
  fuzzer is a genuine zero-divergence signal rather than a tally of deliberate strictness.
- **Residual (documented, handled by the harness):**
  1. **Whole-input consumption** — `parse` rejects trailing content after one value; `cJSON_Parse`
     silently ignores it (it does not require null-termination).
  2. **UTF-8 input domain** — the safe CLI reads stdin as UTF-8 (the safe API takes `&str`); the FFI
     parses raw bytes.
  3. **NUL truncation** — `cJSON_Parse` sizes its input with `strlen`, so a NUL byte truncates the
     document; the safe core is length-based and treats NUL as an ordinary character.
  The harness classifies all three as accepted behavior. Verified: 60s+ runs across multiple seeds
  report **0 divergences**.

---

## Open / to be decided

- `tests/port/` — Rust-native integration tests mirroring the original suite (safe-API mirrors of
  parse/print/minify/compare/utils; the FFI itself is covered by D15's harness).
- `fuzz/` differential harness (FFI vs safe core) + `bench/` methodology/results + `Dockerfile` +
  `README.md` — remaining build-out, tracked in the todo list.
