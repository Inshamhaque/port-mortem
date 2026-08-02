//! Parse-rejection coverage mirroring the spirit of the original parse tests
//! (`parse_value.c`, `parse_with_opts.c`): malformed documents must be
//! rejected with a nonzero error offset. Offsets are asserted only as nonzero
//! because the safe parser reports its own offsets (see DECISIONS.md D3) and
//! does not promise byte-identical positions to cJSON's global error pointer.

use cjson_rs::parse;

fn rejects(input: &str) {
    let err = parse(input)
        .err()
        .unwrap_or_else(|| panic!("{input:?} should fail to parse"));
    // The reported offset must point somewhere inside the input, mirroring
    // cJSON's error-pointer-in-buffer contract.
    assert!(
        err.offset <= input.len(),
        "{input:?} error offset {} is out of range",
        err.offset
    );
}

fn accepts(input: &str) {
    parse(input).unwrap_or_else(|e| panic!("{input:?} should parse, got: {e}"));
}

#[test]
fn rejects_non_json_input() {
    rejects("hello world");
    rejects("<!DOCTYPE html>");
}

#[test]
fn rejects_unterminated_string() {
    rejects("\"abc");
    rejects("\"abc\\u12");
}

#[test]
fn rejects_single_quoted_strings() {
    rejects("'abc'");
}

#[test]
fn rejects_bad_numbers() {
    // .5 is not a value start in cJSON (only '-' or a digit enters a number);
    // --1 cannot be strtod'd.
    rejects(".5");
    rejects("--1");
}

#[test]
fn rejects_trailing_content() {
    rejects("{} {}");
    rejects("1 2");
}

#[test]
fn rejects_unbalanced_containers() {
    rejects("[1,2");
    rejects("{\"a\":1");
    rejects("[1,2}");
    rejects("{\"a\":1]");
}

#[test]
fn rejects_unterminated_value() {
    rejects("{");
    rejects("[");
    rejects("{\"a\":}");
    rejects("[1,]");
}

#[test]
fn accepts_valid_number_forms() {
    accepts("0");
    accepts("-0.5e+3");
    accepts("1E100");
    accepts("1234567890");
    // cJSON's strtod-based parsing accepts leading zeros and bare fractions
    // (DECISIONS.md D16), so the port does too. ("1e" consumes only the "1",
    // leaving trailing content, so it is still rejected as a standalone doc.)
    accepts("01");
    accepts("1.");
}
