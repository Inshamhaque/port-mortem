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

pub use error::Error;
pub use value::{
    compare, create_double_array, create_float_array, create_int_array, create_string_array, duplicate, Kind, Member,
    Value,
};
pub use parser::parse;
pub use minify::minify;

/// cJSON's default maximum nesting level. The parser enforces this when
/// parsing deeply nested documents.
pub const NESTING_LIMIT: usize = 1_000;
