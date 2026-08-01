//! JSON values and the value-level behavior shared by every later module.
//!
//! cJSON preserves object member order and permits duplicate member names.
//! `Vec<Member>` preserves both properties, unlike a map.

/// The JSON kind of a [`Value`]. `Raw` is retained for cJSON compatibility:
/// it represents pre-serialized JSON supplied by a caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Invalid,
    Null,
    Bool, 
    Number,
    String,
    Array,
    Object,
    Raw,
}

/// A named value in a JSON object.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub value: Value,
}

impl Member {
    pub fn new(name: impl Into<String>, value: Value) -> Self {
        Self { name: name.into(), value }
    }
}

/// The safe internal representation of a cJSON node.
///
/// There are no parent, next, or previous pointers here. `Vec` gives Rust
/// ownership of children, and the future FFI layer will adapt this model to
/// cJSON's public linked-node representation when needed.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Invalid,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<Member>),
    Raw(String),
}

impl Value {
    pub const fn null() -> Self { Self::Null }
    pub const fn boolean(value: bool) -> Self { Self::Bool(value) }
    pub const fn number(value: f64) -> Self { Self::Number(value) }
    pub fn string(value: impl Into<String>) -> Self { Self::String(value.into()) }
    pub fn raw(value: impl Into<String>) -> Self { Self::Raw(value.into()) }
    pub fn array(values: impl IntoIterator<Item = Value>) -> Self { Self::Array(values.into_iter().collect()) }
    pub fn object(members: impl IntoIterator<Item = Member>) -> Self { Self::Object(members.into_iter().collect()) }

    pub const fn kind(&self) -> Kind {
        match self {
            Self::Invalid => Kind::Invalid,
            Self::Null => Kind::Null,
            Self::Bool(_) => Kind::Bool,
            Self::Number(_) => Kind::Number,
            Self::String(_) => Kind::String,
            Self::Array(_) => Kind::Array,
            Self::Object(_) => Kind::Object,
            Self::Raw(_) => Kind::Raw,
        }
    }

    pub const fn is_null(&self) -> bool { matches!(self, Self::Null) }
    pub const fn is_bool(&self) -> bool { matches!(self, Self::Bool(_)) }
    pub const fn is_number(&self) -> bool { matches!(self, Self::Number(_)) }
    pub const fn is_string(&self) -> bool { matches!(self, Self::String(_)) }
    pub const fn is_array(&self) -> bool { matches!(self, Self::Array(_)) }
    pub const fn is_object(&self) -> bool { matches!(self, Self::Object(_)) }
    pub const fn is_raw(&self) -> bool { matches!(self, Self::Raw(_)) }

    pub fn as_bool(&self) -> Option<bool> { match self { Self::Bool(value) => Some(*value), _ => None } }
    pub fn as_number(&self) -> Option<f64> { match self { Self::Number(value) => Some(*value), _ => None } }
    pub fn as_str(&self) -> Option<&str> { match self { Self::String(value) => Some(value), _ => None } }
    pub fn as_array(&self) -> Option<&[Value]> { match self { Self::Array(values) => Some(values), _ => None } }
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> { match self { Self::Array(values) => Some(values), _ => None } }
    pub fn as_object(&self) -> Option<&[Member]> { match self { Self::Object(members) => Some(members), _ => None } }
    pub fn as_object_mut(&mut self) -> Option<&mut Vec<Member>> { match self { Self::Object(members) => Some(members), _ => None } }

    /// Returns the first member with this exact name, matching cJSON's object
    /// lookup behavior for objects with duplicate keys.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.as_object()?.iter().find(|member| member.name == name).map(|member| &member.value)
    }

    /// Returns the first ASCII-case-insensitive member, matching
    /// `cJSON_GetObjectItem`.
    pub fn get_case_insensitive(&self, name: &str) -> Option<&Value> {
        self.as_object()?.iter().find(|member| member.name.eq_ignore_ascii_case(name)).map(|member| &member.value)
    }

    pub fn get_index(&self, index: usize) -> Option<&Value> { self.as_array()?.get(index) }
    pub fn len(&self) -> Option<usize> { match self { Self::Array(values) => Some(values.len()), Self::Object(members) => Some(members.len()), _ => None } }
    pub fn is_empty(&self) -> Option<bool> { self.len().map(|length| length == 0) }
}

/// A deep clone, mirroring `cJSON_Duplicate`.
///
/// `recursive` corresponds to cJSON's `recurse` flag. With it disabled, arrays
/// and objects keep their containers but lose their children, exactly as cJSON
/// duplicates a node without recursing into it.
pub fn duplicate(value: &Value, recursive: bool) -> Value {
    if !recursive {
        return match value {
            Value::Array(_) => Value::Array(Vec::new()),
            Value::Object(_) => Value::Object(Vec::new()),
            other => other.clone(),
        };
    }

    match value {
        Value::Array(values) => Value::Array(values.iter().map(|item| duplicate(item, true)).collect()),
        Value::Object(members) => Value::Object(
            members.iter().map(|member| Member::new(member.name.clone(), duplicate(&member.value, true))).collect(),
        ),
        other => other.clone(),
    }
}

/// Deep structural equality mirroring `cJSON_Compare`.
///
/// Objects are compared as unordered key/value sets: like cJSON, both sides
/// are first sorted by member name (a copy here, since Rust cannot mutate the
/// inputs) and then compared member by member. Numbers compare with
/// `compare_double`, strings compare byte-for-byte, and arrays are compared
/// element by element.
pub fn compare(left: &Value, right: &Value, case_sensitive: bool) -> bool {
    if std::mem::discriminant(left) != std::mem::discriminant(right) {
        return false;
    }

    match (left, right) {
        (Value::Number(a), Value::Number(b)) => compare_double(*a, *b),
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) | (Value::Invalid, Value::Invalid) | (Value::Raw(_), Value::Raw(_)) => true,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| compare(x, y, case_sensitive))
        }
        (Value::Object(a), Value::Object(b)) => {
            let left = sorted_members(a, case_sensitive);
            let right = sorted_members(b, case_sensitive);
            if left.len() != right.len() {
                return false;
            }
            left.iter().zip(right.iter()).all(|(x, y)| {
                if !member_names_equal(&x.name, &y.name, case_sensitive) {
                    return false;
                }
                compare(&x.value, &y.value, case_sensitive)
            })
        }
        _ => true,
    }
}

fn compare_double(left: f64, right: f64) -> bool {
    let max = left.abs().max(right.abs());
    (left - right).abs() <= max * f64::EPSILON
}

fn member_names_equal(left: &str, right: &str, case_sensitive: bool) -> bool {
    if case_sensitive { left == right } else { left.eq_ignore_ascii_case(right) }
}

fn sorted_members(members: &[Member], case_sensitive: bool) -> Vec<&Member> {
    let mut sorted: Vec<&Member> = members.iter().collect();
    sorted.sort_by(|a, b| compare_member_names(&a.name, &b.name, case_sensitive));
    sorted
}

fn compare_member_names(left: &str, right: &str, case_sensitive: bool) -> std::cmp::Ordering {
    if case_sensitive {
        left.cmp(right)
    } else {
        left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase())
    }
}

/// Creates an array from integers, mirroring `cJSON_CreateIntArray`.
pub fn create_int_array(values: &[i32]) -> Value {
    Value::Array(values.iter().map(|value| Value::Number(*value as f64)).collect())
}

/// Creates an array from `f32`s, mirroring `cJSON_CreateFloatArray`.
pub fn create_float_array(values: &[f32]) -> Value {
    Value::Array(values.iter().map(|value| Value::Number(*value as f64)).collect())
}

/// Creates an array from `f64`s, mirroring `cJSON_CreateDoubleArray`.
pub fn create_double_array(values: &[f64]) -> Value {
    Value::Array(values.iter().map(|value| Value::Number(*value)).collect())
}

/// Creates an array of strings, mirroring `cJSON_CreateStringArray`.
pub fn create_string_array(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|value| Value::String((*value).into())).collect())
}

#[cfg(test)]
mod tests {
    use super::{compare, create_double_array, create_float_array, create_int_array, create_string_array, duplicate, Kind, Member, Value};

    #[test]
    fn constructors_expose_the_correct_kind() {
        assert_eq!(Value::null().kind(), Kind::Null);
        assert_eq!(Value::boolean(true).as_bool(), Some(true));
        assert_eq!(Value::number(42.5).as_number(), Some(42.5));
        assert_eq!(Value::string("hello").as_str(), Some("hello"));
        assert!(Value::raw("[1]").is_raw());
    }

    #[test]
    fn objects_preserve_order_and_duplicate_names() {
        let object = Value::object([
            Member::new("name", Value::string("first")),
            Member::new("name", Value::string("second")),
        ]);

        assert_eq!(object.len(), Some(2));
        assert_eq!(object.get("name").and_then(Value::as_str), Some("first"));
    }

    #[test]
    fn cjson_style_lookup_is_ascii_case_insensitive() {
        let object = Value::object([Member::new("Content-Type", Value::string("application/json"))]);
        assert_eq!(object.get_case_insensitive("content-type").and_then(Value::as_str), Some("application/json"));
        assert_eq!(object.get("content-type"), None);
    }

    #[test]
    fn array_access_is_bounds_checked() {
        let array = Value::array([Value::number(1.0)]);
        assert_eq!(array.get_index(0).and_then(Value::as_number), Some(1.0));
        assert_eq!(array.get_index(1), None);
    }

    #[test]
    fn compares_objects_as_unordered_key_sets() {
        let left = Value::object([
            Member::new("a", Value::number(1.0)),
            Member::new("b", Value::string("x")),
        ]);
        let right = Value::object([
            Member::new("b", Value::string("x")),
            Member::new("a", Value::number(1.0)),
        ]);
        assert!(compare(&left, &right, false));
        assert!(compare(&left, &right, true));

        let different_case = Value::object([Member::new("A", Value::number(1.0))]);
        assert!(compare(&Value::object([Member::new("a", Value::number(1.0))]), &different_case, false));
        assert!(!compare(&Value::object([Member::new("a", Value::number(1.0))]), &different_case, true));
    }

    #[test]
    fn compare_rejects_type_and_value_mismatches() {
        assert!(!compare(&Value::number(1.0), &Value::string("1"), false));
        assert!(!compare(&Value::number(1.0), &Value::number(2.0), false));
        assert!(compare(&Value::number(1.0), &Value::number(1.0), false));
        let shorter = Value::array([Value::number(1.0)]);
        let longer = Value::array([Value::number(1.0), Value::number(2.0)]);
        assert!(!compare(&shorter, &longer, false));
    }

    #[test]
    fn duplicate_recurses_and_respects_the_flag() {
        let nested = Value::object([Member::new("a", Value::array([Value::number(1.0)]))]);
        let deep = duplicate(&nested, true);
        assert_eq!(deep, nested);

        let shallow = duplicate(&nested, false);
        assert!(shallow.is_object());
        assert_eq!(shallow.len(), Some(0));
        assert_eq!(duplicate(&Value::number(7.0), false), Value::number(7.0));
    }

    #[test]
    fn builds_primitive_arrays() {
        assert_eq!(create_int_array(&[1, 2, 3]), Value::array([Value::number(1.0), Value::number(2.0), Value::number(3.0)]));
        assert_eq!(create_float_array(&[0.5]), Value::array([Value::number(0.5)]));
        assert_eq!(create_double_array(&[1.5]), Value::array([Value::number(1.5)]));
        assert_eq!(create_string_array(&["a", "b"]), Value::array([Value::string("a"), Value::string("b")]));
    }
}
