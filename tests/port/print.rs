//! Printer golden tests mirroring `tests/original/print_*.c`. cJSON's printer
//! has a specific style — tab indentation, `"key":\tvalue`, arrays inline with
//! `, ` separators (v1.7.19 keeps arrays on one line), and its 15/17-significant-
//! digit number formatting — and the port reproduces it byte-for-byte.

use cjson_rs::{parse, print, print_unformatted};

fn printed_unformatted(json: &str) -> String {
    print_unformatted(&parse(json).unwrap()).unwrap()
}

fn printed_formatted(json: &str) -> String {
    print(&parse(json).unwrap()).unwrap()
}

#[test]
fn compact_output_matches_cjson() {
    assert_eq!(printed_unformatted(r#"{"a":[1,2],"b":"x"}"#), r#"{"a":[1,2],"b":"x"}"#);
    assert_eq!(printed_unformatted(r#"{ "a" : [ 1 , 2 ] }"#), r#"{"a":[1,2]}"#);
    assert_eq!(printed_unformatted("{\n\t\"a\":\t1,\n\t\"b\":\ttrue\n}"), r#"{"a":1,"b":true}"#);
}

#[test]
fn formatted_output_matches_cjson_style() {
    assert_eq!(
        printed_formatted(r#"{"a":[1,2],"b":"x"}"#),
        "{\n\t\"a\":\t[1, 2],\n\t\"b\":\t\"x\"\n}"
    );
}

#[test]
fn numbers_use_cjson_15_17_digit_format() {
    assert_eq!(printed_unformatted("1"), "1");
    assert_eq!(printed_unformatted("1.5"), "1.5");
    assert_eq!(printed_unformatted("1e100"), "1e+100");
    assert_eq!(printed_unformatted("0.0001"), "0.0001");
    assert_eq!(printed_unformatted("-0.5e-3"), "-0.0005");
    // Large integers round-trip through cJSON's dtoa as doubles.
    assert_eq!(printed_unformatted("123456789012345678"), "1.2345678901234568e+17");
}

#[test]
fn strings_escape_control_characters() {
    // A JSON \uXXXX escape parses to a control character and prints back as the
    // same escape. Built via format! to keep the source free of tricky escapes.
    // cJSON escapes chars below 0x20 (plus " and \), but not DEL (0x7f).
    let input = format!("\"\\u0001\"");
    assert_eq!(printed_unformatted(&input), input);
    let input = format!("\"\\u001f\"");
    assert_eq!(printed_unformatted(&input), input);
    // DEL (0x7f) is copied literally by cJSON (only chars < 0x20 are escaped),
    // so the round-trip text differs even though the value is identical.
    let input = format!("\"\\u007f\"");
    assert_eq!(printed_unformatted(&input).as_bytes(), &[b'"', 0x7f, b'"']);
}

#[test]
fn strings_keep_pre_escaped_tokens() {
    // Tabs, newlines and quotes already escaped in the input print back identically.
    let input = r#""a\tb\nc""#;
    assert_eq!(printed_unformatted(input), input);
}

#[test]
fn null_renders_as_null() {
    assert_eq!(printed_unformatted("null"), "null");
}

#[test]
fn print_is_a_stable_round_trip() {
    // parse -> print -> parse must be an identity for the original fixtures.
    for name in ["test1", "test2", "test4", "test5", "test9", "test11"] {
        let input = std::fs::read_to_string(format!("tests/original/inputs/{name}")).unwrap();
        let once = parse(&input).unwrap();
        let twice = print_unformatted(&once).unwrap();
        assert_eq!(parse(&twice).unwrap(), once, "{name} round-trip unstable");
    }
}
