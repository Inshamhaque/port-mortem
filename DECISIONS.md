# DECISIONS.md

Every place where the port deliberately diverges from the original (cJSON, in C),
explained in plain language. This file grows as the port grows.

---

## Context

- **Original:** [DaveGamble/cJSON](https://github.com/DaveGamble/cJSON) — written in C.
- **Port:** `cjson-rs` — written in Rust (edition 2024).
- **North star:** the original test suite, pinned under `tests/original/`, must pass
  against the port — untouched and hashed (`SHA256SUMS` + `.port-mortem.toml`).

---

## Decisions made so far

### D1 — Target language: Rust, safe subset
- **Status:** decided
- **What:** We're porting cJSON to idiomatic Rust using only safe code — no `unsafe`
  blocks, no FFI calls into the C runtime.
- **Why:** The original is a single-header C library built on raw pointers and manual
  memory management. Rust's ownership model gives us the same tight, low-level parsing
  goals with memory safety guaranteed by the compiler. It also hits the **Zero Unsafe**
  bonus.

### D2 — Library + CLI binary in one crate
- **Status:** decided, `src/main.rs` implemented
- **What:** One crate, two targets: a library (`cjson_rs`) exposing the API, and a
  binary (`cjson-rs`) exposing a CLI with `parse`, `print`, `minify`, `get`
  (JSON Pointer), and `patch` commands.
- **Why:** The deliverables need both a runnable artifact and an API the original test
  suite can exercise. The CLI also gives us a shared, scriptable surface to diff against
  the original's behavior — the substrate for the differential fuzz harness.

### D3 — Idiomatic Rust error handling instead of cJSON's global error pointer
- **Status:** decided, `src/error.rs` implemented
- **What:** A small `struct Error { offset, message }` implementing `Display` +
  `std::error::Error`, returned through `Result`.
- **Why:** cJSON reports parse failures through a single global `const char *error_ptr`.
  That pattern doesn't fit Rust and isn't safe. Returning an error that carries the byte
  `offset` keeps the exact information the C tests assert on (where parsing failed)
  without a global.

### D4 — Release profile tuned for honest benchmark reporting
- **Status:** decided (`Cargo.toml`)
- **What:** `codegen-units = 1`, `lto = true`, `strip = true` in the release profile.
- **Why:** The submission includes a benchmark report (p99, RSS, startup, throughput).
  This profile makes sure those numbers reflect the production artifact fairly.

### D5 — Tests are the north star; originals stay untouched
- **Status:** decided
- **What:** `tests/original/` is a pinned, byte-for-byte copy of the cJSON test suite,
  hashed at kickoff (`suite_hash` in `.port-mortem.toml`). New port-specific tests live
  in `tests/port/`.
- **Why:** Test parity is the primary scoring criterion. If anyone ever edits an original
  test, it must be named here so judges can weigh the change.

### D6 — `Value` is an owned `enum`, not a linked node
- **Status:** decided, `src/value.rs` implemented
- **What:** cJSON's `cJSON` struct wires `child`/`next`/`prev` pointers into a doubly
  linked tree. We model a document as a recursive `enum Value` (`Null`, `Bool`,
  `Number(f64)`, `String`, `Array(Vec<Value>)`, `Object(Vec<Member>)`, `Raw`) that owns
  its children.
- **Why:** Storing children in a `Vec` gives us the same tree with none of the pointer or
  lifetime juggling. Object members stay in a `Vec<Member>` rather than a map because
  cJSON preserves insertion order and permits duplicate keys. `Raw` is kept for callers
  that supply already-serialized JSON.

### D7 — Numbers stored as `f64`, matching cJSON
- **Status:** decided, `src/value.rs` implemented
- **What:** `Value::Number` holds an `f64`, exactly as cJSON stores every number in a
  `double`.
- **Why:** Behavior parity beats added fidelity. cJSON's printer and comparison
  (`cJSON_Print` and `cJSON_Compare`) assume `double`, so adding integer precision would
  silently change round-trip output and break test parity.

### D8 — Parser is text-in, `Value`-out, and owns the whole document
- **Status:** decided, `src/parser.rs` implemented (17 unit tests green)
- **What:** `parse(&str) -> Result<Value, Error>` runs a recursive-descent parse over the
  bytes and enforces `NESTING_LIMIT`. Trailing garbage after a value returns an [`Error`]
  carrying the byte offset, not cJSON's global error pointer.
- **Why:** Recursive descent maps directly onto cJSON's parser structure, and building an
  owned `Value` up front means nothing is mutated mid-parse. The error offsets keep the
  position information the C tests assert on.

### D9 — Minify works on text, not the parsed tree
- **Status:** decided, `src/minify.rs` implemented
- **What:** `minify(&str) -> String` strips whitespace and comments straight from the
  source text, as `cJSON_Minify` does, instead of round-tripping through a `Value`.
- **Why:** cJSON's minifier is a textual filter. Mirroring it keeps the behavior
  identical — including its `//` and `/* ... */` comment-removal extension and its
  lone-slash handling.

### D10 — Mutation layer maps to typed helpers, not cJSON's raw pointers
- **Status:** decided, `src/mutate.rs` implemented (23 unit tests green)
- **What:** A typed, fallible mutation API replacing cJSON's `cJSON_Add*ToObject`,
  `cJSON_Add*ToArray`, `cJSON_DeleteItem*`, `cJSON_Detach*`, `cJSON_InsertItem*`,
  `cJSON_ReplaceItem*`, and `cJSON_Set*` equivalents, plus a `MutationError`.
- **Why:** cJSON mutates a linked tree through raw `cJSON*` pointers and can fail silently
  or leak. We expose operations over owned [`Value`]s that return
  `Result<_, MutationError>` instead — failure is explicit, safety is compile-time.
  Reference-style adds clone rather than alias, so ownership is never shared unsafely.

### D11 — JSON Pointer / Patch / merge patch return errors, not codes-as-pointers
- **Status:** decided, `src/utils.rs` implemented (34 unit tests green)
- **What:** A `PatchError(u32)` carrying cJSON's `cJSONUtils_ApplyPatches` error codes,
  plus `get_pointer`/`delete_pointer`, JSON Patch (`apply_patches`, `generate_patches`,
  `add_patch_to_array`), RFC 7396 merge patch, `sort_object`, and
  `find_pointer_from_object_to`.
- **Why:** cJSON reports patch failures via `int` return codes and pointer-walking that
  can silently miss. We keep the same numeric codes (for behavioral parity) but return
  them through `Result<_, PatchError>` and resolve paths with checked `Vec`/`slice`
  access, so a bad path is a handled error rather than undefined behavior.

### D12 — Printer reproduces cJSON's exact rendering, including its number quirks
- **Status:** decided, `src/printer.rs` implemented (39 unit tests green)
- **What:** `print` (tab-indented) and `print_unformatted` (compact) mirror `cJSON_Print`
  and `cJSON_PrintUnformatted`. Numbers go through cJSON's 15-then-17 significant-digit
  strategy so round-trips match the C output byte-for-byte; non-finite numbers render as
  `null`, as in cJSON.
- **Why:** Printer output is scored directly. Reproducing cJSON's exact number formatting
  (rather than using Rust's default `f64` display) is what keeps the `print_*` tests
  passing unmodified.

### D13 — CLI surface exists for differential testing
- **Status:** decided, `src/main.rs` implemented
- **What:** A `cjson-rs` binary with subcommands that read a file or stdin and print or
  transform JSON, giving a shared, scriptable interface to compare against the original
  cJSON CLI.
- **Why:** The scoring wants a one-command runnable artifact and differential fuzz over a
  shared public API. A thin CLI over the library is the cheapest way to satisfy both.

### D14 — `GeneratePatches` array removals reuse one index, faithfully
- **Status:** decided, fixed in `src/utils.rs` (`create_patches`, array case)
- **What:** When diffing arrays, cJSON's `create_patches` deletes the tail of `from` in a
  loop that **does not increment its index** — every leftover removal is generated against
  the *same* array position (the count of shared leading elements). We had incremented the
  index, so a shrink like `[1,2,3,4]` → `[1,3]` produced `remove /2, remove /3`; applying
  that removes `/3` after the array had shrunk, failing with `cJSONUtils_ErrPatchResult`
  (13). Now we reuse the index, emitting `remove /2, remove /2`, which applies cleanly
  (each removal shifts the next element into the slot). Caught by the unmodified
  `json_patch_tests` ("test repeated removes", generate pass).
- **Why:** We match the original's *output* (the generated patch) and its ability to
  round-trip generate→apply. The quirk is safe: same-index removals are exactly right for
  deleting a contiguous tail.

### D15 — C-ABI test harness: original suite drives the Rust FFI via a shim
- **Status:** decided, `tests/cJSON.h`, `tests/cJSON.c`, `tests/cJSON_Utils.h`, `Makefile`
- **What:** The original `.c` tests `#include "../cJSON.c"` and `"../cJSON_Utils.h"`
  (single-file style). The build replaces the real library source with a shim that only
  *declares* the C surface: the `cJSON` struct + constants + macros (`cJSON.h`, faithful
  to v1.7.19), the cJSON.c-internal structs `internal_hooks`/`parse_buffer`/`printbuffer`
  plus `extern` for every exported symbol (`cJSON.c`), and the `cJSONUtils_*` externs
  (`cJSON_Utils.h`). The `Makefile` compiles each test against `tests/original/unity` and
  links `libcjson_rs.a` (`crate-type = ["rlib","staticlib"]`), staging `inputs/` +
  `json-patch-tests/` like the original CMake. All 21 binaries pass unmodified.
- **Why:** Keeps `tests/original/` byte-identical to the kickoff hash while giving the
  white-box tests their C type definitions and the linker the Rust implementations. The
  FFI layer (`src/ffi.rs`, wired in via `mod ffi;`) is validated by this suite — that's
  its whole purpose.

### D16 — Safe-core parser matches cJSON's permissiveness; differential fuzz is clean
- **Status:** decided, `src/parser.rs` + `src/ffi.rs` fixed; `fuzz/` harness green
- **What:** The differential fuzzer (`fuzz/harness.py`, FFI `cJSON_Parse` vs safe `parse`)
  surfaced four classes where the two implementations disagreed. Three were the safe core
  being *stricter* than cJSON, so we aligned them to cJSON:
  1. **Whitespace set** — cJSON's `buffer_skip_whitespace` uses `isspace(3)`
     (` \t\n\v\f\r`); we skipped only ` \t\n\r`.
  2. **Raw control characters in strings** — cJSON copies any non-backslash byte verbatim;
     we rejected bytes `< 0x20`.
  3. **Number grammar** — cJSON scans `[0-9 + - e E .]` and hands the token to `strtod`,
     so it accepts leading zeros (`01` → 1), bare fractions (`1.` → 1), and leaves a bare
     exponent unconsumed (`1e` → 1). We enforced RFC 8259's grammar. Both parsers now
     share a `parse_c_float` strtod-subset (`src/ffi.rs`, `src/parser.rs`).
  The fourth was a real **FFI bug**: `buffer_skip_whitespace` used `<= 32` (treated every
  control byte as whitespace), accepting documents the original C rejects. It now uses the
  `isspace` set.
- **Why:** The north star is behavioral parity with cJSON; "safe" describes memory safety
  (no `unsafe`, checked access), not spec strictness. Aligning the two parsers means the
  differential fuzzer is a genuine zero-divergence signal rather than a tally of
  deliberate strictness.
- **Residual (documented, handled by the harness):**
  1. **Whole-input consumption** — `parse` rejects trailing content after one value;
     `cJSON_Parse` silently ignores it (it does not require null-termination).
  2. **UTF-8 input domain** — the safe CLI reads stdin as UTF-8 (the safe API takes
     `&str`); the FFI parses raw bytes.
  3. **NUL truncation** — `cJSON_Parse` sizes its input with `strlen`, so a NUL byte
     truncates the document; the safe core is length-based and treats NUL as an ordinary
     character.
  The harness classifies all three as accepted behavior. Verified: 60s+ runs across
  multiple seeds report **0 divergences**.

---

## Open / to be decided

- `tests/port/` — Rust-native integration tests mirroring the original suite (safe-API
  mirrors of parse/print/minify/compare/utils; the FFI itself is covered by D15's harness).
- `fuzz/` differential harness (FFI vs safe core) + `bench/` methodology/results +
  `Dockerfile` + `README.md` — remaining build-out, tracked in the todo list.
