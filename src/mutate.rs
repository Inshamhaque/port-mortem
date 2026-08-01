//! Safe mutation operations corresponding to cJSON's mutable-tree API.

use std::fmt;

use crate::{Kind, Member, Value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationError {
    WrongContainer { expected: &'static str, actual: Kind },
    MissingItem,
    NotAString,
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongContainer { expected, actual } => write!(formatter, "expected {expected}, found {actual:?}"),
            Self::MissingItem => formatter.write_str("item does not exist"),
            Self::NotAString => formatter.write_str("value is not a string"),
        }
    }
}

impl std::error::Error for MutationError {}

pub fn add_item_to_array(array: &mut Value, item: Value) -> Result<(), MutationError> {
    let actual = array.kind();
    let values = array.as_array_mut().ok_or(MutationError::WrongContainer { expected: "an array", actual })?;
    values.push(item);
    Ok(())
}

/// Adds an independent clone of `item`.
///
/// cJSON's reference APIs share a raw child pointer; safe Rust cannot expose
/// that aliasing and still permit arbitrary mutation, so the native API makes
/// the ownership boundary explicit with a deep clone. The FFI adapter will
/// preserve the C reference flag separately.
pub fn add_item_reference_to_array(array: &mut Value, item: &Value) -> Result<(), MutationError> {
    add_item_to_array(array, item.clone())
}

pub fn add_item_to_object<'a>(object: &'a mut Value, name: impl Into<String>, item: Value) -> Result<&'a mut Value, MutationError> {
    let actual = object.kind();
    let members = object.as_object_mut().ok_or(MutationError::WrongContainer { expected: "an object", actual })?;
    members.push(Member::new(name, item));
    Ok(&mut members.last_mut().expect("member was just pushed").value)
}

pub fn add_item_reference_to_object<'a>(object: &'a mut Value, name: impl Into<String>, item: &Value) -> Result<&'a mut Value, MutationError> {
    add_item_to_object(object, name, item.clone())
}

pub fn add_null_to_object(object: &mut Value, name: impl Into<String>) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::Null)
}

pub fn add_bool_to_object(object: &mut Value, name: impl Into<String>, value: bool) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::Bool(value))
}

pub fn add_number_to_object(object: &mut Value, name: impl Into<String>, value: f64) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::Number(value))
}

pub fn add_string_to_object(object: &mut Value, name: impl Into<String>, value: impl Into<String>) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::String(value.into()))
}

pub fn add_raw_to_object(object: &mut Value, name: impl Into<String>, value: impl Into<String>) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::Raw(value.into()))
}

pub fn add_object_to_object(object: &mut Value, name: impl Into<String>) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::Object(Vec::new()))
}

pub fn add_array_to_object(object: &mut Value, name: impl Into<String>) -> Result<&mut Value, MutationError> {
    add_item_to_object(object, name, Value::Array(Vec::new()))
}

pub fn detach_from_array(array: &mut Value, index: usize) -> Result<Value, MutationError> {
    let actual = array.kind();
    let values = array.as_array_mut().ok_or(MutationError::WrongContainer { expected: "an array", actual })?;
    if index >= values.len() { return Err(MutationError::MissingItem); }
    Ok(values.remove(index))
}

pub fn delete_from_array(array: &mut Value, index: usize) -> Result<(), MutationError> {
    detach_from_array(array, index).map(|_| ())
}

pub fn detach_from_object(object: &mut Value, name: &str, case_sensitive: bool) -> Result<Value, MutationError> {
    let actual = object.kind();
    let members = object.as_object_mut().ok_or(MutationError::WrongContainer { expected: "an object", actual })?;
    let index = members.iter().position(|member| key_matches(&member.name, name, case_sensitive)).ok_or(MutationError::MissingItem)?;
    Ok(members.remove(index).value)
}

pub fn delete_from_object(object: &mut Value, name: &str, case_sensitive: bool) -> Result<(), MutationError> {
    detach_from_object(object, name, case_sensitive).map(|_| ())
}

/// Inserts before `index`; indexes beyond the final element append, just as
/// `cJSON_InsertItemInArray` does.
pub fn insert_in_array(array: &mut Value, index: usize, item: Value) -> Result<(), MutationError> {
    let actual = array.kind();
    let values = array.as_array_mut().ok_or(MutationError::WrongContainer { expected: "an array", actual })?;
    values.insert(index.min(values.len()), item);
    Ok(())
}

pub fn replace_in_array(array: &mut Value, index: usize, replacement: Value) -> Result<Value, MutationError> {
    let actual = array.kind();
    let values = array.as_array_mut().ok_or(MutationError::WrongContainer { expected: "an array", actual })?;
    let item = values.get_mut(index).ok_or(MutationError::MissingItem)?;
    Ok(std::mem::replace(item, replacement))
}

/// Replaces the first matching object member while retaining its existing key,
/// matching cJSON's `ReplaceItemInObject*` behavior.
pub fn replace_in_object(object: &mut Value, name: &str, replacement: Value, case_sensitive: bool) -> Result<Value, MutationError> {
    let actual = object.kind();
    let members = object.as_object_mut().ok_or(MutationError::WrongContainer { expected: "an object", actual })?;
    let member = members.iter_mut().find(|member| key_matches(&member.name, name, case_sensitive)).ok_or(MutationError::MissingItem)?;
    Ok(std::mem::replace(&mut member.value, replacement))
}

pub fn set_number(value: &mut Value, number: f64) -> f64 {
    *value = Value::Number(number);
    number
}

/// Changes an existing JSON string. Other value kinds are left untouched.
pub fn set_string(value: &mut Value, replacement: impl Into<String>) -> Result<&str, MutationError> {
    let Value::String(string) = value else { return Err(MutationError::NotAString); };
    *string = replacement.into();
    Ok(string)
}

pub fn set_bool(value: &mut Value, boolean: bool) -> Result<(), MutationError> {
    let Value::Bool(current) = value else { return Err(MutationError::WrongContainer { expected: "a boolean", actual: value.kind() }); };
    *current = boolean;
    Ok(())
}

fn key_matches(key: &str, sought: &str, case_sensitive: bool) -> bool {
    if case_sensitive { key == sought } else { key.eq_ignore_ascii_case(sought) }
}

#[cfg(test)]
mod tests {
    use crate::{
        add_item_reference_to_array, add_item_to_array, add_number_to_object, add_string_to_object, delete_from_array,
        detach_from_object, insert_in_array, replace_in_array, replace_in_object, set_bool,
        set_number, set_string, MutationError, Value,
    };

    #[test]
    fn adds_items_to_arrays_and_objects() {
        let mut array = Value::array([]);
        add_item_to_array(&mut array, Value::number(1.0)).unwrap();
        assert_eq!(array, Value::array([Value::number(1.0)]));

        let mut object = Value::object([]);
        add_number_to_object(&mut object, "count", 2.0).unwrap();
        add_string_to_object(&mut object, "name", "Ada").unwrap();
        assert_eq!(object.get("count").and_then(Value::as_number), Some(2.0));
        assert_eq!(object.get("name").and_then(Value::as_str), Some("Ada"));
    }

    #[test]
    fn reference_adds_clone_safely() {
        let source = Value::array([Value::number(1.0)]);
        let mut destination = Value::array([]);
        add_item_reference_to_array(&mut destination, &source).unwrap();
        destination.as_array_mut().unwrap()[0].as_array_mut().unwrap().push(Value::number(2.0));
        assert_eq!(source, Value::array([Value::number(1.0)]));
    }

    #[test]
    fn detaches_and_deletes_the_first_matching_member() {
        let mut object = Value::object([
            crate::Member::new("name", Value::string("first")),
            crate::Member::new("name", Value::string("second")),
        ]);
        assert_eq!(detach_from_object(&mut object, "NAME", false), Ok(Value::string("first")));

        let mut array = Value::array([Value::number(1.0), Value::number(2.0)]);
        delete_from_array(&mut array, 0).unwrap();
        assert_eq!(array, Value::array([Value::number(2.0)]));
    }

    #[test]
    fn inserts_and_replaces_items() {
        let mut array = Value::array([Value::number(1.0), Value::number(3.0)]);
        insert_in_array(&mut array, 1, Value::number(2.0)).unwrap();
        assert_eq!(replace_in_array(&mut array, 2, Value::number(4.0)), Ok(Value::number(3.0)));
        assert_eq!(array, Value::array([Value::number(1.0), Value::number(2.0), Value::number(4.0)]));

        let mut object = Value::object([crate::Member::new("name", Value::string("old"))]);
        assert_eq!(replace_in_object(&mut object, "NAME", Value::string("new"), false), Ok(Value::string("old")));
        assert_eq!(object.get("name").and_then(Value::as_str), Some("new"));
    }

    #[test]
    fn updates_scalars_without_changing_their_container() {
        let mut number = Value::Null;
        assert_eq!(set_number(&mut number, 42.0), 42.0);
        assert_eq!(number, Value::number(42.0));

        let mut string = Value::string("old");
        assert_eq!(set_string(&mut string, "new"), Ok("new"));
        let mut boolean = Value::boolean(false);
        set_bool(&mut boolean, true).unwrap();
        assert_eq!(boolean, Value::boolean(true));
    }

    #[test]
    fn rejects_wrong_container_types_and_missing_items() {
        assert_eq!(add_item_to_array(&mut Value::Null, Value::Null), Err(MutationError::WrongContainer { expected: "an array", actual: crate::Kind::Null }));
        assert_eq!(replace_in_array(&mut Value::array([]), 0, Value::Null), Err(MutationError::MissingItem));
    }
}
