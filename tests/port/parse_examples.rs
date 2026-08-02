//! Mirror of `tests/original/parse_examples.c`: the unmodified original input
//! files must parse and round-trip through the printer to their `.expected`
//! files byte-for-byte, and the two intentionally-invalid files must be
//! rejected. The only difference is that this drives the safe `parse`/`print`
//! API instead of the C `cJSON_Parse`/`cJSON_Print` FFI surface.

use cjson_rs::{parse, print};

#[path = "common.rs"]
mod common;

#[test]
fn parse_examples_round_trip_to_expected_output() {
    // test6 is intentionally invalid HTML; test12 is an incomplete object.
    for name in [
        "test1", "test2", "test3", "test4", "test5", "test7", "test8", "test9", "test10", "test11",
    ] {
        let input = common::input(name);
        let expected = common::input(&format!("{name}.expected"));
        let value = parse(&input).unwrap_or_else(|e| panic!("{name} should parse: {e}"));
        let actual = print(&value).unwrap_or_else(|| panic!("{name} should print"));
        assert_eq!(actual, expected, "{name} diverges from the original cJSON_Print output");
    }
}

#[test]
fn test6_is_rejected() {
    // test6 is a Heroku error page, not JSON. The original asserts the parse
    // fails and that the error pointer lands inside the input.
    let input = common::input("test6");
    let err = parse(&input).expect_err("test6 is not JSON and must fail to parse");
    assert!(err.offset <= input.len(), "error offset {} out of range", err.offset);
}

#[test]
fn incomplete_object_is_rejected() {
    // test12 from parse_examples.c: an unterminated object literal.
    let err = parse("{ \"name\": ").expect_err("incomplete JSON must fail to parse");
    assert!(err.offset > 0);
}
