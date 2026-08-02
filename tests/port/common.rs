//! Shared helpers for the port test suite. Integration tests run with the
//! crate root as the working directory, so the untouched original fixtures
//! under `tests/original/` are reachable directly.

use std::fs;

/// Read a file from the unmodified original test suite.
pub fn read_fixture(path: &str) -> String {
    fs::read_to_string(format!("tests/original/{path}"))
        .unwrap_or_else(|e| panic!("failed to read tests/original/{path}: {e}"))
}

/// Read one of the `parse_examples` inputs (e.g. `input("test1")`).
pub fn input(name: &str) -> String {
    read_fixture(&format!("inputs/{name}"))
}
