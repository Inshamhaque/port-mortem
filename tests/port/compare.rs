//! Mirror of `tests/original/compare_tests.c`: cJSON_Compare semantics on the
//! safe API — numbers compare by value regardless of textual form, objects are
//! unordered key-sets by default, and key case sensitivity is opt-in.

use cjson_rs::{compare, parse};

fn cmp(left: &str, right: &str, case_sensitive: bool) -> bool {
    let left = parse(left).unwrap();
    let right = parse(right).unwrap();
    compare(&left, &right, case_sensitive)
}

#[test]
fn nulls_compare_equal() {
    assert!(cmp("null", "null", true));
    assert!(cmp("null", "null", false));
}

#[test]
fn numbers_compare_by_value_ignoring_format() {
    assert!(cmp("1", "1", true));
    assert!(cmp("0.0001", "0.0001", false));
    assert!(cmp("1E100", "10E99", false));
    assert!(!cmp("0.5E-100", "0.5E-101", false));
    assert!(!cmp("1", "2", true));
}

#[test]
fn bools_compare_exactly() {
    assert!(cmp("true", "true", true));
    assert!(cmp("false", "false", true));
    assert!(!cmp("true", "false", true));
}

#[test]
fn strings_compare_exactly() {
    assert!(cmp("\"a\"", "\"a\"", true));
    assert!(!cmp("\"a\"", "\"b\"", true));
}

#[test]
fn arrays_are_order_sensitive() {
    assert!(cmp("[1,2,3]", "[1,2,3]", true));
    assert!(!cmp("[1,2,3]", "[3,2,1]", true));
    assert!(!cmp("[1,2]", "[1,2,3]", true));
}

#[test]
fn objects_are_unordered_key_sets() {
    assert!(cmp("{\"a\":1,\"b\":2}", "{\"b\":2,\"a\":1}", true));
}

#[test]
fn object_keys_respect_case_sensitivity_flag() {
    assert!(cmp("{\"a\":1}", "{\"A\":1}", false));
    assert!(!cmp("{\"a\":1}", "{\"A\":1}", true));
}

#[test]
fn nested_documents_compare_recursively() {
    assert!(cmp("{\"a\":[1,{\"b\":true}]}", "{\"a\":[1,{\"b\":true}]}", true));
    assert!(!cmp("{\"a\":[1,{\"b\":true}]}", "{\"a\":[1,{\"b\":false}]}", true));
}

#[test]
fn different_types_never_compare_equal() {
    assert!(!cmp("null", "1", true));
    assert!(!cmp("true", "1", true));
    assert!(!cmp("\"1\"", "1", true));
    assert!(!cmp("[]", "{}", true));
}
