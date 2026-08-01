//! A safe Rust port of cJSON's JSON parser, printer, mutation, and utility
//! layers.
//!
//! The crate models a JSON document as a [`Value`] tree. Parsing, rendering,
//! minification, mutation, JSON Pointer, JSON Patch, and merge patch all
//! operate on that safe model.

mod error;
mod parser;
mod value;
mod minify;
mod mutate;
mod utils;

pub use error::Error;
pub use value::{
    compare, create_double_array, create_float_array, create_int_array, create_string_array, duplicate, Kind, Member,
    Value,
};
pub use mutate::{
    add_array_to_object, add_bool_to_object, add_item_reference_to_array, add_item_reference_to_object,
    add_item_to_array, add_item_to_object, add_null_to_object, add_number_to_object, add_object_to_object,
    add_raw_to_object, add_string_to_object, delete_from_array, delete_from_object, detach_from_array,
    detach_from_object, insert_in_array, replace_in_array, replace_in_object, set_bool, set_number,
    set_string, MutationError,
};
pub use utils::{
    add_patch_to_array, apply_patches, apply_patches_case_sensitive, delete_pointer, find_pointer_from_object_to,
    generate_merge_patch, generate_merge_patch_case_sensitive, generate_patches, generate_patches_case_sensitive,
    get_pointer, get_pointer_case_sensitive, merge_patch, merge_patch_case_sensitive, sort_object,
    sort_object_case_sensitive, PatchError,
};
pub use parser::parse;
pub use minify::minify;

/// cJSON's default maximum nesting level. The parser enforces this when
/// parsing deeply nested documents.
pub const NESTING_LIMIT: usize = 1_000;
