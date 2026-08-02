//! Mirror of the cJSON_Utils tests (`old_utils_tests.c`, `json_patch_tests.c`,
//! `misc_utils_tests.c`): JSON Pointer resolution with escapes, pointer
//! back-mapping, object sorting, and RFC 6902 patch / RFC 7396 merge-patch
//! round-trips on the safe API.

use cjson_rs::{
    apply_patches, compare, find_pointer_from_object_to, generate_merge_patch, generate_patches,
    get_pointer_case_sensitive, merge_patch, parse, print_unformatted, sort_object_case_sensitive,
    Value,
};

// From old_utils_tests.c: a document whose keys exercise every pointer escape.
const POINTER_ROOT: &str = r#"{"foo":["bar","baz"],"":0,"a/b":1,"c%d":2,"e^f":3,"g|h":4,"i\\j":5,"k\"l":6," ":7,"m~n":8}"#;

fn value(json: &str) -> Value {
    parse(json).unwrap()
}

#[test]
fn get_pointer_resolves_escaped_tokens() {
    let root = value(POINTER_ROOT);
    // "" is the document itself; "/" is the empty member name; ~0/~1 decode ~ and /.
    assert_eq!(get_pointer_case_sensitive(&root, ""), Some(&root));
    assert_eq!(get_pointer_case_sensitive(&root, "/foo"), Some(&value(r#"["bar","baz"]"#)));
    assert_eq!(get_pointer_case_sensitive(&root, "/foo/0"), Some(&value(r#""bar""#)));
    assert_eq!(get_pointer_case_sensitive(&root, "/foo/1"), Some(&value(r#""baz""#)));
    assert_eq!(get_pointer_case_sensitive(&root, "/"), Some(&value("0")));
    assert_eq!(get_pointer_case_sensitive(&root, "/a~1b"), Some(&value("1")));
    assert_eq!(get_pointer_case_sensitive(&root, "/c%d"), Some(&value("2")));
    assert_eq!(get_pointer_case_sensitive(&root, "/e^f"), Some(&value("3")));
    assert_eq!(get_pointer_case_sensitive(&root, "/g|h"), Some(&value("4")));
    assert_eq!(get_pointer_case_sensitive(&root, "/i\\j"), Some(&value("5")));
    assert_eq!(get_pointer_case_sensitive(&root, "/k\"l"), Some(&value("6")));
    assert_eq!(get_pointer_case_sensitive(&root, "/ "), Some(&value("7")));
    assert_eq!(get_pointer_case_sensitive(&root, "/m~0n"), Some(&value("8")));
}

#[test]
fn get_pointer_missing_paths_return_none() {
    let root = value(POINTER_ROOT);
    assert_eq!(get_pointer_case_sensitive(&root, "/missing"), None);
    assert_eq!(get_pointer_case_sensitive(&root, "/foo/2"), None);
    assert_eq!(get_pointer_case_sensitive(&root, "/foo/x"), None);
}

#[test]
fn find_pointer_backmaps_to_paths() {
    let root = value(r#"{"numbers":[0,1,2,3,4,5,6,7,8,9]}"#);
    let nums = get_pointer_case_sensitive(&root, "/numbers").unwrap();
    let num6 = get_pointer_case_sensitive(&root, "/numbers/6").unwrap();
    assert_eq!(find_pointer_from_object_to(&root, num6), Some("/numbers/6".to_string()));
    assert_eq!(find_pointer_from_object_to(&root, nums), Some("/numbers".to_string()));
    assert_eq!(find_pointer_from_object_to(&root, &root), Some(String::new()));
}

#[test]
fn sort_object_orders_members() {
    let mut object = value(r#"{"banana":1,"apple":2,"cherry":3}"#);
    sort_object_case_sensitive(&mut object);
    assert_eq!(print_unformatted(&object).unwrap(), r#"{"apple":2,"banana":1,"cherry":3}"#);
}

#[test]
fn generated_patches_apply_and_reach_target() {
    // Includes the "test repeated removes" shrink from json-patch-tests/tests.json
    // that caught the D14 divergence, plus object add/replace/remove cases.
    let cases: &[(&str, &str)] = &[
        ("[1,2,3,4]", "[1,3]"),
        (r#"{"foo":"bar"}"#, r#"{"baz":"qux","foo":"bar"}"#),
        (r#"{"foo":["bar","baz"]}"#, r#"{"foo":["bar","qux","baz"]}"#),
        (r#"{"foo":["bar","qux","baz"]}"#, r#"{"foo":["bar","baz"]}"#),
        (r#"{"a":1,"b":2}"#, r#"{"b":2}"#),
        ("[1,2,3]", "[]"),
        ("{}", r#"{"x":[1]}"#),
    ];
    for (from, to) in cases {
        let from_value = value(from);
        let to_value = value(to);
        let mut object = from_value.clone();
        let patches = generate_patches(&from_value, &to_value, true);
        apply_patches(&mut object, &patches, true)
            .unwrap_or_else(|e| panic!("apply failed for {from} -> {to}: {e:?}"));
        assert!(
            compare(&object, &to_value, true),
            "patch for {from} -> {to} did not reach target (object order may differ)"
        );
    }
}

#[test]
fn merge_patch_round_trips_merge_and_generate() {
    // Merge a patch into a target, then regenerate a merge patch back to the merge result.
    let target = value(r#"{"title":"Goodbye!","author":{"givenName":"John","familyName":"Doe"},"tags":["api","testing"],"content":"This will be unchanged"}"#);
    let patch = value(r#"{"title":"Hello!","phoneNumber":"+01-123-456-7890","author":{"familyName":null},"tags":["api"]}"#);
    let merged = merge_patch(&target, &patch, true);

    assert_eq!(
        print_unformatted(&merged).unwrap(),
        r#"{"title":"Hello!","author":{"givenName":"John"},"tags":["api"],"content":"This will be unchanged","phoneNumber":"+01-123-456-7890"}"#
    );

    // generate_merge_patch(target, merged) must yield a patch that merges back to `merged`.
    let back = generate_merge_patch(&target, &merged, true);
    assert!(compare(&merge_patch(&target, &back, true), &merged, true));
}

#[test]
fn merge_patch_adds_removes_and_recurses() {
    // remove member via null, add member, mutate nested object.
    assert_eq!(
        print_unformatted(&merge_patch(&value(r#"{"a":1,"b":2}"#), &value(r#"{"b":null}"#), true)).unwrap(),
        r#"{"a":1}"#
    );
    assert_eq!(
        print_unformatted(&merge_patch(&value("{}"), &value(r#"{"x":1}"#), true)).unwrap(),
        r#"{"x":1}"#
    );
}
