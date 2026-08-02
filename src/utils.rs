//! The utility layer: JSON Pointer, JSON Patch, merge patch, and object
//! sorting — the Rust take on cJSON's `cJSON_Utils`.

use crate::{compare, duplicate, Member, Value};

/// A patch error, matching the codes cJSON's `cJSONUtils_ApplyPatches` returns.
/// `0` is success; anything else is a specific failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PatchError(pub u32);

/// The `op` field of a JSON Patch object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchOperation {
    Add,
    Remove,
    Replace,
    Move,
    Copy,
    Test,
}

/// Chops a pointer into its path tokens, dropping the empty segment that the
/// leading `/` creates.
fn split_pointer(pointer: &str) -> Vec<&str> {
    pointer.split('/').skip(1).collect()
}

/// Reads an array index token. Leading zeros and non-digits are rejected, just
/// like cJSON's `decode_array_index_from_pointer`.
fn decode_array_index(token: &str) -> Option<usize> {
    if token.is_empty() || (token.starts_with('0') && token.len() > 1) {
        return None;
    }
    if !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// Checks whether one pointer token matches a member name, decoding `~0`/`~1`
/// as it goes — the same way cJSON's `compare_pointers` works.
fn token_matches_name(name: &str, token: &str, case_sensitive: bool) -> bool {
    let mut names = name.chars();
    let mut tokens = token.chars().peekable();
    loop {
        match (names.next(), tokens.next()) {
            (None, None) => return true,
            (Some(n), Some(t)) if t == '~' => match tokens.next() {
                Some('0') if n == '~' => {}
                Some('1') if n == '/' => {}
                _ => return false,
            },
            (Some(n), Some(t)) => {
                if case_sensitive {
                    if n != t {
                        return false;
                    }
                } else if n.to_ascii_lowercase() != t.to_ascii_lowercase() {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// Turns `~0`/`~1` escapes back into the real member name.
fn decode_pointer_inplace(token: &str) -> String {
    let mut output = String::with_capacity(token.len());
    let mut characters = token.chars();
    while let Some(character) = characters.next() {
        if character == '~' {
            match characters.next() {
                Some('0') => output.push('~'),
                Some('1') => output.push('/'),
                _ => break,
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// Encodes a member name so it can live safely inside a pointer token.
fn encode_string_as_pointer(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '/' => output.push_str("~1"),
            '~' => output.push_str("~0"),
            character => output.push(character),
        }
    }
    output
}

fn member_name_matches(left: &str, right: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        left == right
    } else {
        left.eq_ignore_ascii_case(right)
    }
}

fn compare_member_names(left: &str, right: &str, case_sensitive: bool) -> std::cmp::Ordering {
    if case_sensitive {
        left.cmp(right)
    } else {
        left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase())
    }
}

fn compare_double(left: f64, right: f64) -> bool {
    let max = left.abs().max(right.abs());
    (left - right).abs() <= max * f64::EPSILON
}

/// Follows `pointer` and returns the value it lands on, or `None` if the path
/// dead-ends. The Rust version of `cJSONUtils_GetPointer`.
pub fn get_pointer<'a>(value: &'a Value, pointer: &str, case_sensitive: bool) -> Option<&'a Value> {
    let mut current = value;
    for token in split_pointer(pointer) {
        current = match current {
            Value::Array(values) => values.get(decode_array_index(token)?)?,
            Value::Object(members) => {
                members.iter().find(|member| token_matches_name(&member.name, token, case_sensitive))?.value_ref()
            }
            _ => return None,
        };
    }
    Some(current)
}

/// Same as [`get_pointer`], but treats member names as case-sensitive.
pub fn get_pointer_case_sensitive<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    get_pointer(value, pointer, true)
}

/// Like [`get_pointer`], but hands back a mutable reference so callers can edit
/// the value the pointer resolves to.
fn get_pointer_mut<'a>(value: &'a mut Value, pointer: &str, case_sensitive: bool) -> Option<&'a mut Value> {
    let mut current = value;
    for token in split_pointer(pointer) {
        let next: &mut Value = match current {
            Value::Array(values) => values.get_mut(decode_array_index(token)?)?,
            Value::Object(members) => {
                let member = members.iter_mut().find(|member| token_matches_name(&member.name, token, case_sensitive))?;
                &mut member.value
            }
            _ => return None,
        };
        current = next;
    }
    Some(current)
}

impl Member {
    fn value_ref(&self) -> &Value {
        &self.value
    }
}

/// Splits a pointer into the parent path and the last child token. If the
/// pointer has no `/` at all, both come back `None`.
fn split_parent_child(pointer: &str) -> (Option<&str>, Option<&str>) {
    match pointer.rfind('/') {
        Some(position) => (Some(&pointer[..position]), Some(&pointer[position + 1..])),
        None => (None, None),
    }
}

/// Detaches (removes and returns) the value at `pointer`. This backs the
/// remove and move operations.
fn detach_path(value: &mut Value, pointer: &str, case_sensitive: bool) -> Result<Value, PatchError> {
    let (parent_pointer, child_token) = split_parent_child(pointer);
    let child_token = child_token.ok_or(PatchError(9))?;
    let parent = get_pointer_mut(value, parent_pointer.unwrap_or(""), case_sensitive).ok_or(PatchError(13))?;
    let child = decode_pointer_inplace(child_token);

    match parent {
        Value::Array(values) => {
            let index = decode_array_index(&child).ok_or(PatchError(11))?;
            if index >= values.len() {
                return Err(PatchError(13));
            }
            Ok(values.remove(index))
        }
        Value::Object(members) => {
            let index = members
                .iter()
                .position(|member| member_name_matches(&member.name, &child, case_sensitive))
                .ok_or(PatchError(13))?;
            Ok(members.remove(index).value)
        }
        _ => Err(PatchError(9)),
    }
}

/// Deletes whatever sits at `pointer`. This is the pointer-mutation delete path.
pub fn delete_pointer(value: &mut Value, pointer: &str, case_sensitive: bool) -> Result<(), PatchError> {
    detach_path(value, pointer, case_sensitive).map(|_| ())
}

/// Sorts an object's members by name — `cJSONUtils_SortObject`.
pub fn sort_object(value: &mut Value, case_sensitive: bool) {
    if let Value::Object(members) = value {
        members.sort_by(|left, right| compare_member_names(&left.name, &right.name, case_sensitive));
    }
}

/// Same as [`sort_object`], but case-sensitive.
pub fn sort_object_case_sensitive(value: &mut Value) {
    sort_object(value, true);
}

fn object_item<'a>(object: &'a Value, name: &str, case_sensitive: bool) -> Option<&'a Value> {
    match object {
        Value::Object(members) => members
            .iter()
            .find(|member| member_name_matches(&member.name, name, case_sensitive))
            .map(|member| &member.value),
        _ => None,
    }
}

fn decode_patch_operation(patch: &Value, case_sensitive: bool) -> Option<PatchOperation> {
    let operation = object_item(patch, "op", case_sensitive)?.as_str()?;
    match operation {
        "add" => Some(PatchOperation::Add),
        "remove" => Some(PatchOperation::Remove),
        "replace" => Some(PatchOperation::Replace),
        "move" => Some(PatchOperation::Move),
        "copy" => Some(PatchOperation::Copy),
        "test" => Some(PatchOperation::Test),
        _ => None,
    }
}

fn apply_patch(object: &mut Value, patch: &Value, case_sensitive: bool) -> Result<(), PatchError> {
    let path = object_item(patch, "path", case_sensitive)
        .and_then(Value::as_str)
        .ok_or(PatchError(2))?;
    let opcode = decode_patch_operation(patch, case_sensitive).ok_or(PatchError(3))?;

    if opcode == PatchOperation::Test {
        let value = object_item(patch, "value", case_sensitive).ok_or(PatchError(7))?;
        let at = get_pointer(object, path, case_sensitive);
        let equal = at.is_some_and(|found| compare(found, value, case_sensitive));
        return if equal { Ok(()) } else { Err(PatchError(1)) };
    }

    if path.is_empty() {
        match opcode {
            PatchOperation::Remove => {
                *object = Value::Invalid;
                return Ok(());
            }
            PatchOperation::Replace | PatchOperation::Add => {
                let value = object_item(patch, "value", case_sensitive).ok_or(PatchError(7))?;
                *object = duplicate(value, true);
                return Ok(());
            }
            _ => {}
        }
    }

    if matches!(opcode, PatchOperation::Remove | PatchOperation::Replace) {
        let _old = detach_path(object, path, case_sensitive)?;
        if opcode == PatchOperation::Remove {
            return Ok(());
        }
    }

    let value = if matches!(opcode, PatchOperation::Move | PatchOperation::Copy) {
        let from = object_item(patch, "from", case_sensitive)
            .and_then(Value::as_str)
            .ok_or(PatchError(4))?;
        if opcode == PatchOperation::Move {
            detach_path(object, from, case_sensitive)?
        } else {
            let source = get_pointer(object, from, case_sensitive).ok_or(PatchError(5))?;
            duplicate(source, true)
        }
    } else {
        let value = object_item(patch, "value", case_sensitive).ok_or(PatchError(7))?;
        duplicate(value, true)
    };

    let (parent_pointer, child_token) = split_parent_child(path);
    let child_token = child_token.ok_or(PatchError(9))?;
    let parent = get_pointer_mut(object, parent_pointer.unwrap_or(""), case_sensitive).ok_or(PatchError(9))?;
    let child = decode_pointer_inplace(child_token);

    match parent {
        Value::Array(values) => {
            if child == "-" {
                values.push(value);
            } else {
                let index = decode_array_index(&child).ok_or(PatchError(11))?;
                if index > values.len() {
                    return Err(PatchError(10));
                }
                values.insert(index, value);
            }
        }
        Value::Object(members) => {
            members.retain(|member| !member_name_matches(&member.name, &child, case_sensitive));
            members.push(Member::new(child, value));
        }
        _ => return Err(PatchError(9)),
    }

    Ok(())
}

/// Applies a list of JSON Patch operations in order — `cJSONUtils_ApplyPatches`.
pub fn apply_patches(object: &mut Value, patches: &Value, case_sensitive: bool) -> Result<(), PatchError> {
    let Value::Array(operations) = patches else {
        return Err(PatchError(1));
    };
    for operation in operations {
        apply_patch(object, operation, case_sensitive)?;
    }
    Ok(())
}

/// Same as [`apply_patches`], but case-sensitive.
pub fn apply_patches_case_sensitive(object: &mut Value, patches: &Value) -> Result<(), PatchError> {
    apply_patches(object, patches, true)
}

/// Builds a single patch object and appends it to the output — `compose_patch`.
fn compose_patch(out: &mut Vec<Value>, operation: &str, path: &str, suffix: Option<&str>, value: Option<&Value>) {
    let mut members = vec![Member::new("op", Value::string(operation))];
    let full_path = match suffix {
        Some(suffix) => format!("{path}/{}", encode_string_as_pointer(suffix)),
        None => path.to_string(),
    };
    members.push(Member::new("path", Value::string(full_path)));
    if let Some(value) = value {
        members.push(Member::new("value", duplicate(value, true)));
    }
    out.push(Value::Object(members));
}

/// Adds one already-formed patch to a patch array — `cJSONUtils_AddPatchToArray`.
pub fn add_patch_to_array(out: &mut Vec<Value>, operation: &str, path: &str, value: Option<&Value>) {
    compose_patch(out, operation, path, None, value);
}

fn push_replace(out: &mut Vec<Value>, path: &str, value: &Value) {
    compose_patch(out, "replace", path, None, Some(value));
}

fn push_remove(out: &mut Vec<Value>, path: &str, suffix: &str) {
    compose_patch(out, "remove", path, Some(suffix), None);
}

fn push_add(out: &mut Vec<Value>, path: &str, suffix: &str, value: &Value) {
    compose_patch(out, "add", path, Some(suffix), Some(value));
}

fn sorted_members<'a>(members: &'a [Member], case_sensitive: bool) -> Vec<&'a Member> {
    let mut sorted: Vec<&Member> = members.iter().collect();
    sorted.sort_by(|left, right| compare_member_names(&left.name, &right.name, case_sensitive));
    sorted
}

fn create_patches(out: &mut Vec<Value>, path: &str, from: &Value, to: &Value, case_sensitive: bool) {
    if from.kind() != to.kind() {
        push_replace(out, path, to);
        return;
    }

    match (from, to) {
        (Value::Number(left), Value::Number(right)) => {
            if !compare_double(*left, *right) {
                push_replace(out, path, to);
            }
        }
        (Value::String(left), Value::String(right)) => {
            if left != right {
                push_replace(out, path, to);
            }
        }
        (Value::Array(from_values), Value::Array(to_values)) => {
            let shared = from_values.len().min(to_values.len());
            for index in 0..shared {
                create_patches(out, &format!("{path}/{index}"), &from_values[index], &to_values[index], case_sensitive);
            }
            // Faithful to cJSON_Utils.c `create_patches`: the remove-leftover
            // loop does NOT increment `index`, so every leftover removal targets
            // the same position (`shared`). Each removal shifts the next element
            // down into that slot, so a contiguous tail is deleted correctly and
            // the patch applies cleanly.
            for _ in shared..from_values.len() {
                push_remove(out, path, &shared.to_string());
            }
            for index in shared..to_values.len() {
                push_add(out, path, "-", &to_values[index]);
            }
        }
        (Value::Object(from_members), Value::Object(to_members)) => {
            let from_sorted = sorted_members(from_members, case_sensitive);
            let to_sorted = sorted_members(to_members, case_sensitive);
            let (mut i, mut j) = (0, 0);
            while i < from_sorted.len() || j < to_sorted.len() {
                let diff = if i >= from_sorted.len() {
                    std::cmp::Ordering::Greater
                } else if j >= to_sorted.len() {
                    std::cmp::Ordering::Less
                } else {
                    compare_member_names(&from_sorted[i].name, &to_sorted[j].name, case_sensitive)
                };

                match diff {
                    std::cmp::Ordering::Equal => {
                        let new_path = format!("{path}/{}", encode_string_as_pointer(&from_sorted[i].name));
                        create_patches(out, &new_path, &from_sorted[i].value, &to_sorted[j].value, case_sensitive);
                        i += 1;
                        j += 1;
                    }
                    std::cmp::Ordering::Less => {
                        push_remove(out, path, &from_sorted[i].name);
                        i += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        push_add(out, path, &to_sorted[j].name, &to_sorted[j].value);
                        j += 1;
                    }
                }
            }
        }
        _ => {}
    }
}

/// Computes the JSON Patch that turns `from` into `to` — `cJSONUtils_GeneratePatches`.
pub fn generate_patches(from: &Value, to: &Value, case_sensitive: bool) -> Value {
    let mut out = Vec::new();
    create_patches(&mut out, "", from, to, case_sensitive);
    Value::Array(out)
}

/// Same as [`generate_patches`], but case-sensitive.
pub fn generate_patches_case_sensitive(from: &Value, to: &Value) -> Value {
    generate_patches(from, to, true)
}

/// Finds a pointer that reaches `target` from `object`, or `None` if it isn't
/// reachable — `cJSONUtils_FindPointerFromObjectTo`.
pub fn find_pointer_from_object_to(object: &Value, target: &Value) -> Option<String> {
    if std::ptr::eq(object, target) {
        return Some(String::new());
    }

    match object {
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                if let Some(sub) = find_pointer_from_object_to(child, target) {
                    return Some(format!("/{index}{sub}"));
                }
            }
            None
        }
        Value::Object(members) => {
            for member in members {
                if let Some(sub) = find_pointer_from_object_to(&member.value, target) {
                    return Some(format!("/{}{sub}", encode_string_as_pointer(&member.name)));
                }
            }
            None
        }
        _ => None,
    }
}

/// Applies an RFC 7396 merge patch — `cJSONUtils_MergePatch`.
pub fn merge_patch(target: &Value, patch: &Value, case_sensitive: bool) -> Value {
    let Value::Object(patch_members) = patch else {
        return duplicate(patch, true);
    };

    let mut members: Vec<Member> = match target {
        Value::Object(members) => members.clone(),
        _ => Vec::new(),
    };

    for patch_member in patch_members {
        if patch_member.value.is_null() {
            members.retain(|member| !member_name_matches(&member.name, &patch_member.name, case_sensitive));
        } else {
            let existing = members
                .iter()
                .find(|member| member_name_matches(&member.name, &patch_member.name, case_sensitive))
                .map(|member| &member.value);
            let replacement = merge_patch(existing.unwrap_or(&Value::Invalid), &patch_member.value, case_sensitive);
            if let Some(index) = members
                .iter()
                .position(|member| member_name_matches(&member.name, &patch_member.name, case_sensitive))
            {
                members[index].value = replacement;
            } else {
                members.push(Member::new(patch_member.name.clone(), replacement));
            }
        }
    }

    Value::Object(members)
}

/// Same as [`merge_patch`], but case-sensitive.
pub fn merge_patch_case_sensitive(target: &Value, patch: &Value) -> Value {
    merge_patch(target, patch, true)
}

/// Builds the RFC 7396 merge patch that turns `from` into `to` —
/// `cJSONUtils_GenerateMergePatch`. An empty object means nothing needs to change.
pub fn generate_merge_patch(from: &Value, to: &Value, case_sensitive: bool) -> Value {
    if !matches!(to, Value::Object(_)) || !matches!(from, Value::Object(_)) {
        return duplicate(to, true);
    }

    let Value::Object(from_members) = from else { unreachable!() };
    let Value::Object(to_members) = to else { unreachable!() };
    let from_sorted = sorted_members(from_members, case_sensitive);
    let to_sorted = sorted_members(to_members, case_sensitive);

    let mut out: Vec<Member> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < from_sorted.len() || j < to_sorted.len() {
        let diff = if i >= from_sorted.len() {
            std::cmp::Ordering::Greater
        } else if j >= to_sorted.len() {
            std::cmp::Ordering::Less
        } else {
            from_sorted[i].name.cmp(&to_sorted[j].name)
        };

        match diff {
            std::cmp::Ordering::Less => {
                out.push(Member::new(from_sorted[i].name.clone(), Value::Null));
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(Member::new(to_sorted[j].name.clone(), duplicate(&to_sorted[j].value, true)));
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if !compare(&from_sorted[i].value, &to_sorted[j].value, case_sensitive) {
                    out.push(Member::new(
                        to_sorted[j].name.clone(),
                        generate_merge_patch(&from_sorted[i].value, &to_sorted[j].value, case_sensitive),
                    ));
                }
                i += 1;
                j += 1;
            }
        }
    }

    Value::Object(out)
}

/// Same as [`generate_merge_patch`], but case-sensitive.
pub fn generate_merge_patch_case_sensitive(from: &Value, to: &Value) -> Value {
    generate_merge_patch(from, to, true)
}

#[cfg(test)]
mod tests {
    use crate::{parse, utils::*, Member, Value};

    #[test]
    fn resolves_object_and_array_pointer_tokens() {
        let value = parse(r#"{"a":{"b":[10,20,30]},"c":7}"#).unwrap();
        assert_eq!(get_pointer(&value, "/a/b/1", false).and_then(Value::as_number), Some(20.0));
        assert_eq!(get_pointer(&value, "/c", false).and_then(Value::as_number), Some(7.0));
        assert_eq!(get_pointer(&value, "", false).map(Value::is_object), Some(true));
        assert_eq!(get_pointer(&value, "/missing", false), None);
        assert_eq!(get_pointer(&value, "/a/b/99", false), None);
    }

    #[test]
    fn pointer_case_sensitivity_and_escapes() {
        let value = parse(r#"{"A/B":1}"#).unwrap();
        assert_eq!(get_pointer(&value, "/a~1b", false).and_then(Value::as_number), Some(1.0));
        assert_eq!(get_pointer_case_sensitive(&value, "/a~1b"), None);

        let tilde = parse(r#"{"a~b":2}"#).unwrap();
        assert_eq!(get_pointer(&tilde, "/a~0b", false).and_then(Value::as_number), Some(2.0));
    }

    #[test]
    fn deletes_an_array_element_and_object_member() {
        let mut value = parse(r#"{"a":[1,2,3]}"#).unwrap();
        delete_pointer(&mut value, "/a/1", false).unwrap();
        assert_eq!(parse(r#"{"a":[1,3]}"#).unwrap(), value);

        let mut object = parse(r#"{"x":1,"y":2}"#).unwrap();
        delete_pointer(&mut object, "/x", false).unwrap();
        assert_eq!(parse(r#"{"y":2}"#).unwrap(), object);
    }

    #[test]
    fn sorts_object_members_by_name() {
        let mut value = parse(r#"{"zeta":1,"Alpha":2,"mid":3}"#).unwrap();
        sort_object(&mut value, false);
        let Value::Object(members) = &value else { panic!("expected object") };
        let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "mid", "zeta"]);
    }

    #[test]
    fn applies_patch_operations() {
        let mut value = parse(r#"{"a":1,"b":[1,2]}"#).unwrap();
        let patches = parse(r#"[
            {"op":"add","path":"/c","value":true},
            {"op":"replace","path":"/a","value":9},
            {"op":"remove","path":"/b/0"},
            {"op":"test","path":"/c","value":true}
        ]"#).unwrap();
        apply_patches(&mut value, &patches, false).unwrap();
        assert!(crate::compare(&value, &parse(r#"{"a":9,"b":[2],"c":true}"#).unwrap(), false));
    }

    #[test]
    fn move_and_copy_patch_operations() {
        let mut moved = parse(r#"{"a":1,"b":2}"#).unwrap();
        let move_patch = parse(r#"[{"op":"move","from":"/a","path":"/b"}]"#).unwrap();
        apply_patches(&mut moved, &move_patch, false).unwrap();
        assert_eq!(parse(r#"{"b":1}"#).unwrap(), moved);

        let mut copied = parse(r#"{"a":[1]}"#).unwrap();
        let copy_patch = parse(r#"[{"op":"copy","from":"/a","path":"/b"}]"#).unwrap();
        apply_patches(&mut copied, &copy_patch, false).unwrap();
        assert_eq!(parse(r#"{"a":[1],"b":[1]}"#).unwrap(), copied);
    }

    #[test]
    fn patch_reports_errors_with_cjson_codes() {
        let mut value = parse(r#"{"a":1}"#).unwrap();
        let bad = parse(r#"[{"op":"remove","path":"/nope"}]"#).unwrap();
        assert_eq!(apply_patches(&mut value, &bad, false), Err(PatchError(13)));

        let unknown_op = parse(r#"[{"op":"explode","path":"/a"}]"#).unwrap();
        assert_eq!(apply_patches(&mut value, &unknown_op, false), Err(PatchError(3)));
    }

    #[test]
    fn generates_patches_between_documents() {
        let from = parse(r#"{"a":1,"b":[1,2],"keep":"same"}"#).unwrap();
        let to = parse(r#"{"a":2,"b":[1],"keep":"same","new":true}"#).unwrap();
        let patches = generate_patches(&from, &to, false);
        let mut result = duplicate(&from, true);
        apply_patches(&mut result, &patches, false).unwrap();
        assert!(crate::compare(&result, &to, false));
    }

    #[test]
    fn merge_patch_removes_adds_and_recurses() {
        let target = parse(r#"{"a":{"x":1,"y":2},"b":3}"#).unwrap();
        let patch = parse(r#"{"a":{"x":9},"b":null,"c":true}"#).unwrap();
        let result = merge_patch(&target, &patch, false);
        assert_eq!(parse(r#"{"a":{"x":9,"y":2},"c":true}"#).unwrap(), result);
    }

    #[test]
    fn generates_merge_patch_that_round_trips() {
        let from = parse(r#"{"a":1,"b":2,"c":3}"#).unwrap();
        let to = parse(r#"{"a":1,"b":9,"d":4}"#).unwrap();
        let patch = generate_merge_patch(&from, &to, false);
        assert_eq!(merge_patch(&from, &patch, false), to);
    }

    #[test]
    fn finds_pointer_from_object_to_value() {
        let mut value = parse(r#"{"a":{"b":[1,2]}}"#).unwrap();
        let target = get_pointer(&value, "/a/b/1", false).unwrap();
        assert_eq!(find_pointer_from_object_to(&value, target), Some("/a/b/1".into()));

        let fresh = Value::number(5.0);
        assert_eq!(find_pointer_from_object_to(&mut value, &fresh), None);
    }
}
