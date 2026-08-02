//! Mirror of `tests/original/minify_tests.c`: cJSON_Minify strips insignificant
//! whitespace and /* */ and // comments, keeps whitespace inside strings, and
//! never overflows on unterminated comments or a dangling escape.

use cjson_rs::minify;

#[test]
fn unclosed_multiline_comment_is_stripped() {
    assert_eq!(minify("/* bla"), "");
}

#[test]
fn pending_escape_survives() {
    // A string literal with a dangling backslash is left alone.
    assert_eq!(minify(r#""\"#), r#""\"#);
}

#[test]
fn removes_single_line_comments() {
    let input = "{// this is {} \"some kind\" of [] comment /*, don't you see\n}";
    assert_eq!(minify(input), "{}");
}

#[test]
fn removes_spaces_tabs_and_crlf() {
    assert_eq!(minify("{ \"key\":\ttrue\r\n    }"), "{\"key\":true}");
}

#[test]
fn removes_multiline_comments() {
    let input = "{/* this is\n a /* multi\n //line \n {comment \"\\\" */}";
    assert_eq!(minify(input), "{}");
}

#[test]
fn does_not_modify_string_contents() {
    // Whitespace and escaped quotes inside a JSON string are preserved.
    let input = r#""this is a string \" \t bla""#;
    assert_eq!(minify(input), input);
}

#[test]
fn empty_input_and_trivial_documents() {
    assert_eq!(minify(""), "");
    assert_eq!(minify("{}"), "{}");
    assert_eq!(minify("{ \"key\" : true }"), "{\"key\":true}");
}
