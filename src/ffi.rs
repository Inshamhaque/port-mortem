//! C-ABI compatibility layer that lets the original C test suite exercise the
//! Rust port.
//!
//! cJSON's tests do two things a pure-Rust API cannot serve directly: they call
//! functions with the exact C names and signatures from `cJSON.h` /
//! `cJSON_Utils.h`, and they poke the raw `cJSON` struct fields (`->child`,
//! `->next`, `->prev`, `->string`, `->valuestring`, ...). This module provides
//! that C surface, implemented in Rust:
//!
//! * A [`cJSON`] struct with the exact `#[repr(C)]` layout of the C one, so
//!   tests that hand-build linked lists on the stack keep working.
//! * `#[unsafe(no_mangle)] extern "C"` exports for every public function (plus the
//!   internal white-box helpers the unit tests reach for directly).
//! * Real linked-node memory, allocated through the libc allocator that cJSON
//!   uses (`global_hooks`), so C-side `free()` and `cJSON_Delete` round-trip
//!   without leaks or double-frees.
//!
//! # Direction of trust
//!
//! This module *implements* the C API in Rust. The C tests *call into* it. It
//! never links the original `cJSON.c`; the test build replaces that file with a
//! tiny stub that only declares types and externs. Everything the tests observe
//! is produced by this Rust code.
//!
//! # Safety containment
//!
//! Every `unsafe` block in the crate lives in this file. The safe core
//! (`value.rs`, `parser.rs`, `printer.rs`, `utils.rs`) is untouched. The
//! internal parse/print walkers are ported faithfully from `cJSON.c` because the
//! white-box tests drive them through C-visible `parse_buffer`/`printbuffer`
//! structs, which the safe `&str`-based API cannot model. The `cJSON_Utils`
//! layer is the one place that delegates to the safe [`Value`] model, via the
//! node ⇄ value converters below.

#![allow(unsafe_code)] // this file is the crate's single, documented unsafe zone
// In edition 2024 every unsafe operation needs its own `unsafe` block even
// inside an `unsafe fn`; a whole-file shim would drown in noise, so the lint is
// suppressed here and the module boundary is the safety argument.
#![allow(unsafe_op_in_unsafe_fn)]

use core::ffi::{c_char, c_double, c_int, c_void};
use core::ptr;

use crate::{Member, Value};

// ---------------------------------------------------------------------------
// C types and constants
// ---------------------------------------------------------------------------

/// `typedef int cJSON_bool;` (kept lowercase to match the C identifier)
#[allow(non_camel_case_types)]
type cJSON_bool = c_int;
/// `size_t` on all supported platforms is `usize`.

/// The `type` bit flags, matching `cJSON.h`.
const CJSON_INVALID: c_int = 0;
const CJSON_FALSE: c_int = 1 << 0;
const CJSON_TRUE: c_int = 1 << 1;
const CJSON_NULL: c_int = 1 << 2;
const CJSON_NUMBER: c_int = 1 << 3;
const CJSON_STRING: c_int = 1 << 4;
const CJSON_ARRAY: c_int = 1 << 5;
const CJSON_OBJECT: c_int = 1 << 6;
const CJSON_RAW: c_int = 1 << 7;
const CJSON_IS_REFERENCE: c_int = 256;
const CJSON_STRING_IS_CONST: c_int = 512;

/// `CJSON_NESTING_LIMIT` from `cJSON.h`.
const CJSON_NESTING_LIMIT: usize = 1_000;
/// `CJSON_CIRCULAR_LIMIT` from `cJSON.h`.
const CJSON_CIRCULAR_LIMIT: usize = 10_000;

/// The public node type. Layout must match `cJSON.h` byte-for-byte.
///
/// Field order, sizes and alignment are what matter; the Rust names are
/// internal (the C header owns the names the tests use).
#[repr(C)]
pub struct cJSON {
    next: *mut cJSON,
    prev: *mut cJSON,
    child: *mut cJSON,
    ctype: c_int,
    valuestring: *mut c_char,
    valueint: c_int,
    valuedouble: c_double,
    string: *mut c_char,
}

/// `cJSON_Hooks` from `cJSON.h`.
#[repr(C)]
pub struct cJSON_Hooks {
    pub malloc_fn: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
    pub free_fn: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// `internal_hooks` from `cJSON.c`. Three C function pointers.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct internal_hooks {
    pub allocate: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
    pub deallocate: Option<unsafe extern "C" fn(*mut c_void)>,
    pub reallocate: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
}

/// `parse_buffer` from `cJSON.c`. Tests allocate these by value on the stack.
#[repr(C)]
pub struct parse_buffer {
    pub content: *const u8,
    pub length: usize,
    pub offset: usize,
    pub depth: usize,
    pub hooks: internal_hooks,
}

/// `printbuffer` from `cJSON.c`.
#[repr(C)]
pub struct printbuffer {
    pub buffer: *mut u8,
    pub length: usize,
    pub offset: usize,
    pub depth: usize,
    pub noalloc: cJSON_bool,
    pub format: cJSON_bool,
    pub hooks: internal_hooks,
}

/// `error` from `cJSON.c`: the last parse-error location.
#[repr(C)]
#[derive(Clone, Copy)]
struct error {
    json: *const u8,
    position: usize,
}

// ---------------------------------------------------------------------------
// Global state and the allocator
// ---------------------------------------------------------------------------

// The system allocator, exactly as `cJSON.c` links it.
unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
    fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void;
}

/// The allocator in force. Tests read and even mutate this via `reset()` and
/// `cJSON_InitHooks`, so it is a real exported symbol.
#[unsafe(no_mangle)]
pub static mut global_hooks: internal_hooks = internal_hooks {
    allocate: Some(malloc as unsafe extern "C" fn(usize) -> *mut c_void),
    deallocate: Some(free as unsafe extern "C" fn(*mut c_void)),
    reallocate: Some(realloc as unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void),
};

/// Last parse-error location; backing store for `cJSON_GetErrorPtr`.
#[allow(non_upper_case_globals)] // mirrors the C identifier
static mut global_error: error = error { json: ptr::null(), position: 0 };

unsafe fn hook_allocate(size: usize) -> *mut c_void {
    match global_hooks.allocate {
        Some(allocate) => allocate(size),
        None => ptr::null_mut(),
    }
}

unsafe fn hook_deallocate(pointer: *mut c_void) {
    if let Some(deallocate) = global_hooks.deallocate {
        deallocate(pointer);
    }
}

#[allow(dead_code)] // faithful port of cJSON.c's reallocate helper
unsafe fn hook_reallocate(pointer: *mut c_void, size: usize) -> *mut c_void {
    match global_hooks.reallocate {
        Some(reallocate) => reallocate(pointer, size),
        None => ptr::null_mut(),
    }
}

/// Length of a NUL-terminated C string (strlen). Returns 0 for NULL.
unsafe fn c_strlen(pointer: *const u8) -> usize {
    if pointer.is_null() {
        return 0;
    }
    let mut length = 0;
    while *pointer.add(length) != 0 {
        length += 1;
    }
    length
}

/// `strcmp` over two NUL-terminated C strings.
unsafe fn c_strcmp(a: *const u8, b: *const u8) -> c_int {
    let mut index = 0usize;
    loop {
        let left = *a.add(index);
        let right = *b.add(index);
        if left != right {
            return left as c_int - right as c_int;
        }
        if left == 0 {
            return 0;
        }
        index += 1;
    }
}

/// Copy a NUL-terminated C string through the active allocator (cJSON_strdup).
unsafe fn c_json_strdup_impl(string: *const u8, hooks: *const internal_hooks) -> *mut u8 {
    if string.is_null() {
        return ptr::null_mut();
    }
    let length = c_strlen(string) + 1;
    let hooks = if hooks.is_null() {
        ptr::addr_of!(global_hooks)
    } else {
        hooks
    };
    let allocate = match (*hooks).allocate {
        Some(allocate) => allocate,
        None => return ptr::null_mut(),
    };
    let copy = allocate(length) as *mut u8;
    if copy.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(string, copy, length);
    copy
}

// ---------------------------------------------------------------------------
// Node lifecycle
// ---------------------------------------------------------------------------

/// Allocate and zero a node (`cJSON_New_Item`).
unsafe fn new_item() -> *mut cJSON {
    let node = hook_allocate(size_of::<cJSON>()) as *mut cJSON;
    if !node.is_null() {
        ptr::write_bytes(node as *mut u8, 0, size_of::<cJSON>());
    }
    node
}

/// Free a single node and its strings, honouring the reference/const flags.
/// This is the recursive-free core of `cJSON_Delete`.
unsafe fn delete_node(mut node: *mut cJSON) {
    while !node.is_null() {
        let next = (*node).next;
        if ((*node).ctype & CJSON_IS_REFERENCE) == 0 && !(*node).child.is_null() {
            delete_node((*node).child);
        }
        if ((*node).ctype & CJSON_IS_REFERENCE) == 0 && !(*node).valuestring.is_null() {
            hook_deallocate((*node).valuestring as *mut c_void);
            (*node).valuestring = ptr::null_mut();
        }
        if ((*node).ctype & CJSON_STRING_IS_CONST) == 0 && !(*node).string.is_null() {
            hook_deallocate((*node).string as *mut c_void);
            (*node).string = ptr::null_mut();
        }
        hook_deallocate(node as *mut c_void);
        node = next;
    }
}

/// Link `item` at the end of `array`'s child list (`add_item_to_array`).
///
/// cJSON keeps `array->child->prev` pointing at the last element for O(1)
/// appends; the first element's `prev` points at itself.
unsafe fn add_item_to_array_impl(array: *mut cJSON, item: *mut cJSON) -> cJSON_bool {
    if item.is_null() || array.is_null() || array == item {
        return 0;
    }
    let child = (*array).child;
    if child.is_null() {
        (*array).child = item;
        (*item).prev = item;
        (*item).next = ptr::null_mut();
    } else if !(*child).prev.is_null() {
        // append to the end: link after the current tail
        (*(*child).prev).next = item;
        (*item).prev = (*child).prev;
        (*item).next = ptr::null_mut();
        (*child).prev = item; // keep head->prev == tail
    }
    1
}

/// Add a (possibly const) key to an object member (`add_item_to_object`).
unsafe fn add_item_to_object_impl(
    object: *mut cJSON,
    string: *const c_char,
    item: *mut cJSON,
    hooks: *const internal_hooks,
    constant_key: cJSON_bool,
) -> cJSON_bool {
    if object.is_null() || string.is_null() || item.is_null() || object == item {
        return 0;
    }

    let new_key: *mut c_char;
    let new_type: c_int;
    if constant_key != 0 {
        new_key = string as *mut c_char;
        new_type = (*item).ctype | CJSON_STRING_IS_CONST;
    } else {
        new_key = c_json_strdup_impl(string as *const u8, hooks) as *mut c_char;
        if new_key.is_null() {
            return 0;
        }
        new_type = (*item).ctype & !CJSON_STRING_IS_CONST;
    }

    if ((*item).ctype & CJSON_STRING_IS_CONST) == 0 && !(*item).string.is_null() {
        let hooks = if hooks.is_null() {
            ptr::addr_of!(global_hooks)
        } else {
            hooks
        };
        if let Some(deallocate) = (*hooks).deallocate {
            deallocate((*item).string as *mut c_void);
        }
    }

    (*item).string = new_key;
    (*item).ctype = new_type;

    add_item_to_array_impl(object, item)
}

/// Create a reference node that aliases `item` (`create_reference`).
unsafe fn create_reference(item: *const cJSON) -> *mut cJSON {
    if item.is_null() {
        return ptr::null_mut();
    }
    let reference = new_item();
    if reference.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(item, reference, 1);
    (*reference).string = ptr::null_mut();
    (*reference).ctype |= CJSON_IS_REFERENCE;
    (*reference).next = ptr::null_mut();
    (*reference).prev = ptr::null_mut();
    reference
}

/// Fetch the child at `index`, or NULL.
unsafe fn get_array_item(array: *const cJSON, index: usize) -> *mut cJSON {
    if array.is_null() {
        return ptr::null_mut();
    }
    let mut current = (*array).child;
    let mut remaining = index;
    while !current.is_null() && remaining > 0 {
        remaining -= 1;
        current = (*current).next;
    }
    current
}

/// Case-insensitive `strcmp`, matching `cJSON.c`.
unsafe fn case_insensitive_strcmp(string1: *const u8, string2: *const u8) -> c_int {
    if string1.is_null() || string2.is_null() {
        return 1;
    }
    let mut a = string1;
    let mut b = string2;
    while *a != 0 && to_lower(*a) == to_lower(*b) {
        a = a.add(1);
        b = b.add(1);
    }
    to_lower(*a) as c_int - to_lower(*b) as c_int
}

fn to_lower(byte: u8) -> u8 {
    if byte.is_ascii_uppercase() {
        byte + (b'a' - b'A')
    } else {
        byte
    }
}

/// Look up an object member (`get_object_item`).
unsafe fn get_object_item(object: *const cJSON, name: *const c_char, case_sensitive: cJSON_bool) -> *mut cJSON {
    if object.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let mut current = (*object).child;
    if case_sensitive != 0 {
        while !current.is_null() && !(*current).string.is_null() && c_strcmp(name as *const u8, (*current).string as *const u8) != 0
        {
            current = (*current).next;
        }
    } else {
        while !current.is_null() && case_insensitive_strcmp(name as *const u8, (*current).string as *const u8) != 0 {
            current = (*current).next;
        }
    }
    if current.is_null() || (*current).string.is_null() {
        return ptr::null_mut();
    }
    current
}

// ---------------------------------------------------------------------------
// Internal parse machinery (ported from cJSON.c)
// ---------------------------------------------------------------------------

unsafe fn can_read(buffer: *const parse_buffer, size: usize) -> bool {
    !buffer.is_null() && (*buffer).offset + size <= (*buffer).length
}

unsafe fn can_access_at_index(buffer: *const parse_buffer, index: usize) -> bool {
    !buffer.is_null() && (*buffer).offset + index < (*buffer).length
}

unsafe fn buffer_at_offset(buffer: *const parse_buffer) -> *const u8 {
    (*buffer).content.add((*buffer).offset)
}

/// Compare `length` bytes against a byte literal.
unsafe fn bytes_match(pointer: *const u8, literal: &[u8]) -> bool {
    core::slice::from_raw_parts(pointer, literal.len()) == literal
}

/// Skip whitespace (bytes <= 0x20), like `cJSON.c`.
unsafe fn buffer_skip_whitespace(buffer: *mut parse_buffer) -> *mut parse_buffer {
    if buffer.is_null() || (*buffer).content.is_null() {
        return ptr::null_mut();
    }
    // matching cJSON.c: an exhausted buffer is left untouched (so callers can
    // detect "ran off the end" via cannot_access_at_index)
    if !can_access_at_index(buffer, 0) {
        return buffer;
    }
    while can_access_at_index(buffer, 0) && *buffer_at_offset(buffer) <= 32 {
        (*buffer).offset += 1;
    }
    if (*buffer).offset == (*buffer).length {
        (*buffer).offset -= 1;
    }
    buffer
}

/// Skip a UTF-8 BOM at the very start of the buffer (`skip_utf8_bom`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skip_utf8_bom(buffer: *mut parse_buffer) -> *mut parse_buffer {
    if buffer.is_null() || (*buffer).content.is_null() || (*buffer).offset != 0 {
        return ptr::null_mut();
    }
    if can_access_at_index(buffer, 4) && bytes_match(buffer_at_offset(buffer), b"\xEF\xBB\xBF") {
        (*buffer).offset += 3;
    }
    buffer
}

/// Decode four hex digits (`parse_hex4`); 0 signals an invalid digit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parse_hex4(input: *const u8) -> u32 {
    let mut value: u32 = 0;
    for i in 0..4 {
        let digit = *input.add(i);
        let nibble = match digit {
            b'0'..=b'9' => (digit - b'0') as u32,
            b'A'..=b'F' => 10 + (digit - b'A') as u32,
            b'a'..=b'f' => 10 + (digit - b'a') as u32,
            _ => return 0,
        };
        value += nibble;
        if i < 3 {
            value <<= 4;
        }
    }
    value
}

/// Convert one `\uXXXX` literal (or surrogate pair) to UTF-8
/// (`utf16_literal_to_utf8`). Writes into `output`, advancing it. Returns the
/// number of input bytes consumed, or 0 on failure.
unsafe fn utf16_literal_to_utf8(input_pointer: *const u8, input_end: *const u8, output: *mut *mut u8) -> u8 {
    if input_end.offset_from(input_pointer) < 6 {
        return 0;
    }
    let first_sequence = input_pointer;
    let first_code = parse_hex4(first_sequence.add(2));

    if (0xDC00..=0xDFFF).contains(&first_code) {
        return 0;
    }

    let mut codepoint: u64;
    let sequence_length: u8;
    if (0xD800..=0xDBFF).contains(&first_code) {
        // UTF-16 surrogate pair
        let second_sequence = first_sequence.add(6);
        if input_end.offset_from(second_sequence) < 6 {
            return 0;
        }
        if *second_sequence != b'\\' || *second_sequence.add(1) != b'u' {
            return 0;
        }
        let second_code = parse_hex4(second_sequence.add(2));
        if !(0xDC00..=0xDFFF).contains(&second_code) {
            return 0;
        }
        sequence_length = 12;
        codepoint = 0x10000 + (((first_code & 0x3FF) as u64) << 10) | (second_code & 0x3FF) as u64;
    } else {
        sequence_length = 6;
        codepoint = first_code as u64;
    }

    let (utf8_length, first_byte_mark): (u8, u64) = if codepoint < 0x80 {
        (1, 0)
    } else if codepoint < 0x800 {
        (2, 0xC0)
    } else if codepoint < 0x10000 {
        (3, 0xE0)
    } else if codepoint <= 0x10FFFF {
        (4, 0xF0)
    } else {
        return 0;
    };

    // Encode big-endian: the leading byte lands at position 0, continuation
    // bytes fill the slots from (utf8_length - 1) down to 1.
    if utf8_length > 1 {
        for position in (1..utf8_length).rev() {
            *((*output).add(position as usize)) = ((codepoint | 0x80) & 0xBF) as u8;
            codepoint >>= 6;
        }
    }
    **output = (codepoint | first_byte_mark) as u8;
    *output = (*output).add(utf8_length as usize);

    sequence_length
}

/// Parse one JSON string from the buffer into `item` (`parse_string`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parse_string(item: *mut cJSON, input_buffer: *mut parse_buffer) -> cJSON_bool {
    if input_buffer.is_null()
        || (*input_buffer).content.is_null()
        || (*input_buffer).length == 0
        || *buffer_at_offset(input_buffer) != b'"'
    {
        return 0;
    }

    let mut input_pointer = buffer_at_offset(input_buffer).add(1);
    let mut input_end = buffer_at_offset(input_buffer).add(1);

    // Measure the raw extent of the string, counting the bytes an escape
    // occupies as a single output byte.
    let allocation_length;
    let mut skipped_bytes = 0usize;
    while input_end.offset_from((*input_buffer).content) < (*input_buffer).length as isize && *input_end != b'"' {
        if *input_end == b'\\' {
            if input_end.add(1).offset_from((*input_buffer).content) >= (*input_buffer).length as isize {
                return 0;
            }
            skipped_bytes += 1;
            input_end = input_end.add(1);
        }
        input_end = input_end.add(1);
    }
    if input_end.offset_from((*input_buffer).content) >= (*input_buffer).length as isize || *input_end != b'"' {
        return 0;
    }
    allocation_length = input_end.offset_from(buffer_at_offset(input_buffer)) as usize - skipped_bytes;

    let output = hook_allocate(allocation_length + 1) as *mut u8;
    if output.is_null() {
        return 0;
    }
    let mut output_pointer = output;

    while input_pointer < input_end {
        if *input_pointer != b'\\' {
            *output_pointer = *input_pointer;
            output_pointer = output_pointer.add(1);
            input_pointer = input_pointer.add(1);
        } else {
            if input_end.offset_from(input_pointer) < 1 {
                hook_deallocate(output as *mut c_void);
                return 0;
            }
            let mut sequence_length: u8 = 2;
            match *input_pointer.add(1) {
                b'b' => {
                    *output_pointer = b'\x08';
                    output_pointer = output_pointer.add(1);
                }
                b'f' => {
                    *output_pointer = b'\x0C';
                    output_pointer = output_pointer.add(1);
                }
                b'n' => {
                    *output_pointer = b'\n';
                    output_pointer = output_pointer.add(1);
                }
                b'r' => {
                    *output_pointer = b'\r';
                    output_pointer = output_pointer.add(1);
                }
                b't' => {
                    *output_pointer = b'\t';
                    output_pointer = output_pointer.add(1);
                }
                b'"' | b'\\' | b'/' => {
                    *output_pointer = *input_pointer.add(1);
                    output_pointer = output_pointer.add(1);
                }
                b'u' => {
                    sequence_length = utf16_literal_to_utf8(input_pointer, input_end, &mut output_pointer);
                    if sequence_length == 0 {
                        hook_deallocate(output as *mut c_void);
                        return 0;
                    }
                }
                _ => {
                    hook_deallocate(output as *mut c_void);
                    return 0;
                }
            }
            input_pointer = input_pointer.add(sequence_length as usize);
        }
    }

    *output_pointer = 0;

    (*item).ctype = CJSON_STRING;
    (*item).valuestring = output as *mut c_char;
    (*input_buffer).offset = input_end.offset_from((*input_buffer).content) as usize;
    (*input_buffer).offset += 1;
    1
}

/// Parse one JSON number from the buffer into `item` (`parse_number`).
///
/// Number text is scanned into a temporary buffer and parsed with strtod-like
/// leniency: an unterminated exponent is not consumed, and overflow yields
/// ±inf which is kept (the surrounding structure rejects the trailing junk).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parse_number(item: *mut cJSON, input_buffer: *mut parse_buffer) -> cJSON_bool {
    if input_buffer.is_null() || (*input_buffer).content.is_null() {
        return 0;
    }

    let mut number_string_length = 0usize;
    let mut i = 0usize;
    while can_access_at_index(input_buffer, i) {
        match *buffer_at_offset(input_buffer).add(i) {
            b'0'..=b'9' | b'+' | b'-' | b'e' | b'E' | b'.' => number_string_length += 1,
            _ => break,
        }
        i += 1;
    }

    // malloc a temporary NUL-terminated copy, as the C code does.
    let number_c_string = hook_allocate(number_string_length + 1) as *mut u8;
    if number_c_string.is_null() {
        return 0;
    }
    if number_string_length > 0 {
        ptr::copy_nonoverlapping(buffer_at_offset(input_buffer), number_c_string, number_string_length);
    }
    *number_c_string.add(number_string_length) = 0;

    let (number, consumed) = match parse_c_float(core::slice::from_raw_parts(number_c_string, number_string_length)) {
        Some(result) => result,
        None => {
            hook_deallocate(number_c_string as *mut c_void);
            return 0;
        }
    };
    hook_deallocate(number_c_string as *mut c_void);

    (*item).valuedouble = number;
    // saturating cast, matching cJSON's explicit INT_MAX/INT_MIN clamps
    (*item).valueint = if number >= 2_147_483_647.0 {
        c_int::MAX
    } else if number <= -2_147_483_648.0 {
        c_int::MIN
    } else {
        number as c_int
    };
    (*item).ctype = CJSON_NUMBER;

    (*input_buffer).offset += consumed;
    1
}

/// The subset of C `strtod` that `parse_number` can produce: optional sign,
/// digit/dot mantissa, optional exponent. Returns `(value, bytes consumed)`.
fn parse_c_float(input: &[u8]) -> Option<(f64, usize)> {
    let mut pos = 0usize;
    if matches!(input.get(pos), Some(b'+' | b'-')) {
        pos += 1;
    }
    let mut digits = 0usize;
    while matches!(input.get(pos), Some(b'0'..=b'9')) {
        pos += 1;
        digits += 1;
    }
    let mut fraction = 0usize;
    if input.get(pos) == Some(&b'.') {
        pos += 1;
        while matches!(input.get(pos), Some(b'0'..=b'9')) {
            pos += 1;
            fraction += 1;
        }
    }
    if digits == 0 && fraction == 0 {
        return None; // strtod does not advance: parse error
    }
    // Exponent. strtod leaves it unconsumed when it carries no digits ("1e" -> 1).
    if matches!(input.get(pos), Some(b'e' | b'E')) {
        let exponent_start = pos;
        pos += 1;
        if matches!(input.get(pos), Some(b'+' | b'-')) {
            pos += 1;
        }
        let exponent_digits = pos;
        while matches!(input.get(pos), Some(b'0'..=b'9')) {
            pos += 1;
        }
        if pos == exponent_digits {
            pos = exponent_start;
        }
    }

    let token = core::str::from_utf8(&input[..pos]).ok()?;
    let value = token.parse::<f64>().ok()?;
    Some((value, pos))
}

/// Parse one value from the buffer (`parse_value`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parse_value(item: *mut cJSON, input_buffer: *mut parse_buffer) -> cJSON_bool {
    if input_buffer.is_null() || (*input_buffer).content.is_null() {
        return 0;
    }

    let at = *buffer_at_offset(input_buffer);
    // null / false / true literals, compared by prefix
    if can_read(input_buffer, 4) && bytes_match(buffer_at_offset(input_buffer), b"null") {
        (*item).ctype = CJSON_NULL;
        (*input_buffer).offset += 4;
        return 1;
    }
    if can_read(input_buffer, 5) && bytes_match(buffer_at_offset(input_buffer), b"false") {
        (*item).ctype = CJSON_FALSE;
        (*input_buffer).offset += 5;
        return 1;
    }
    if can_read(input_buffer, 4) && bytes_match(buffer_at_offset(input_buffer), b"true") {
        (*item).ctype = CJSON_TRUE;
        (*item).valueint = 1;
        (*input_buffer).offset += 4;
        return 1;
    }
    if can_access_at_index(input_buffer, 0) && at == b'"' {
        return parse_string(item, input_buffer);
    }
    if can_access_at_index(input_buffer, 0) && (at == b'-' || (b'0'..=b'9').contains(&at)) {
        return parse_number(item, input_buffer);
    }
    if can_access_at_index(input_buffer, 0) && at == b'[' {
        return parse_array(item, input_buffer);
    }
    if can_access_at_index(input_buffer, 0) && at == b'{' {
        return parse_object(item, input_buffer);
    }
    0
}

/// Parse an array from the buffer (`parse_array`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parse_array(item: *mut cJSON, input_buffer: *mut parse_buffer) -> cJSON_bool {
    if input_buffer.is_null() || (*input_buffer).content.is_null() {
        return 0;
    }
    if (*input_buffer).depth >= CJSON_NESTING_LIMIT {
        return 0;
    }
    (*input_buffer).depth += 1;

    if *buffer_at_offset(input_buffer) != b'[' {
        (*input_buffer).depth -= 1;
        return 0;
    }

    let mut head: *mut cJSON = ptr::null_mut();
    let mut current_item: *mut cJSON = ptr::null_mut();

    (*input_buffer).offset += 1;
    buffer_skip_whitespace(input_buffer);
    if can_access_at_index(input_buffer, 0) && *buffer_at_offset(input_buffer) == b']' {
        (*input_buffer).depth -= 1;
        if !head.is_null() {
            (*head).prev = current_item;
        }
        (*item).ctype = CJSON_ARRAY;
        (*item).child = head;
        (*input_buffer).offset += 1;
        return 1;
    }
    if !can_access_at_index(input_buffer, 0) {
        (*input_buffer).offset -= 1;
        (*input_buffer).depth -= 1;
        return 0;
    }
    (*input_buffer).offset -= 1;

    let mut failed = false;
    loop {
        let new_child = new_item();
        if new_child.is_null() {
            failed = true;
            break;
        }
        if head.is_null() {
            current_item = new_child;
            head = new_child;
        } else {
            (*current_item).next = new_child;
            (*new_child).prev = current_item;
            current_item = new_child;
        }
        (*input_buffer).offset += 1;
        buffer_skip_whitespace(input_buffer);
        if parse_value(current_item, input_buffer) == 0 {
            failed = true;
            break;
        }
        buffer_skip_whitespace(input_buffer);
        if !can_access_at_index(input_buffer, 0) || *buffer_at_offset(input_buffer) != b',' {
            break;
        }
    }

    if failed || !can_access_at_index(input_buffer, 0) || *buffer_at_offset(input_buffer) != b']' {
        if !head.is_null() {
            delete_node(head);
        }
        (*input_buffer).depth -= 1;
        return 0;
    }

    (*input_buffer).depth -= 1;
    if !head.is_null() {
        (*head).prev = current_item;
    }
    (*item).ctype = CJSON_ARRAY;
    (*item).child = head;
    (*input_buffer).offset += 1;
    1
}

/// Parse an object from the buffer (`parse_object`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parse_object(item: *mut cJSON, input_buffer: *mut parse_buffer) -> cJSON_bool {
    if input_buffer.is_null() || (*input_buffer).content.is_null() {
        return 0;
    }
    if (*input_buffer).depth >= CJSON_NESTING_LIMIT {
        return 0;
    }
    (*input_buffer).depth += 1;

    if !can_access_at_index(input_buffer, 0) || *buffer_at_offset(input_buffer) != b'{' {
        (*input_buffer).depth -= 1;
        return 0;
    }

    let mut head: *mut cJSON = ptr::null_mut();
    let mut current_item: *mut cJSON = ptr::null_mut();

    (*input_buffer).offset += 1;
    buffer_skip_whitespace(input_buffer);
    if can_access_at_index(input_buffer, 0) && *buffer_at_offset(input_buffer) == b'}' {
        (*input_buffer).depth -= 1;
        if !head.is_null() {
            (*head).prev = current_item;
        }
        (*item).ctype = CJSON_OBJECT;
        (*item).child = head;
        (*input_buffer).offset += 1;
        return 1;
    }
    if !can_access_at_index(input_buffer, 0) {
        (*input_buffer).offset -= 1;
        (*input_buffer).depth -= 1;
        return 0;
    }
    (*input_buffer).offset -= 1;

    let mut failed = false;
    loop {
        let new_child = new_item();
        if new_child.is_null() {
            failed = true;
            break;
        }
        if head.is_null() {
            current_item = new_child;
            head = new_child;
        } else {
            (*current_item).next = new_child;
            (*new_child).prev = current_item;
            current_item = new_child;
        }
        if !can_access_at_index(input_buffer, 1) {
            failed = true;
            break;
        }
        (*input_buffer).offset += 1;
        buffer_skip_whitespace(input_buffer);
        if parse_string(current_item, input_buffer) == 0 {
            failed = true;
            break;
        }
        buffer_skip_whitespace(input_buffer);
        // swap valuestring/string: the parsed name becomes the member key
        (*current_item).string = (*current_item).valuestring;
        (*current_item).valuestring = ptr::null_mut();

        if !can_access_at_index(input_buffer, 0) || *buffer_at_offset(input_buffer) != b':' {
            failed = true;
            break;
        }
        (*input_buffer).offset += 1;
        buffer_skip_whitespace(input_buffer);
        if parse_value(current_item, input_buffer) == 0 {
            failed = true;
            break;
        }
        buffer_skip_whitespace(input_buffer);
        if !can_access_at_index(input_buffer, 0) || *buffer_at_offset(input_buffer) != b',' {
            break;
        }
    }

    if failed || !can_access_at_index(input_buffer, 0) || *buffer_at_offset(input_buffer) != b'}' {
        if !head.is_null() {
            delete_node(head);
        }
        (*input_buffer).depth -= 1;
        return 0;
    }

    (*input_buffer).depth -= 1;
    if !head.is_null() {
        (*head).prev = current_item;
    }
    (*item).ctype = CJSON_OBJECT;
    (*item).child = head;
    (*input_buffer).offset += 1;
    1
}

// ---------------------------------------------------------------------------
// Internal print machinery (ported from cJSON.c)
// ---------------------------------------------------------------------------

/// Ensure the printbuffer has room for `needed` more bytes and return a
/// pointer to the write position (`ensure`). Advances nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ensure(output_buffer: *mut printbuffer, needed: usize) -> *mut u8 {
    if output_buffer.is_null() || (*output_buffer).buffer.is_null() {
        return ptr::null_mut();
    }
    if (*output_buffer).length > 0 && (*output_buffer).offset >= (*output_buffer).length {
        return ptr::null_mut();
    }
    if needed > c_int::MAX as usize {
        return ptr::null_mut();
    }
    let total = needed + (*output_buffer).offset + 1;
    if total <= (*output_buffer).length {
        return (*output_buffer).buffer.add((*output_buffer).offset);
    }
    if (*output_buffer).noalloc != 0 {
        return ptr::null_mut();
    }

    let newsize: usize;
    if total > (c_int::MAX as usize) / 2 {
        if total <= c_int::MAX as usize {
            newsize = c_int::MAX as usize;
        } else {
            return ptr::null_mut();
        }
    } else {
        newsize = total * 2;
    }

    let new_buffer: *mut u8;
    if let Some(reallocate) = (*output_buffer).hooks.reallocate {
        new_buffer = reallocate((*output_buffer).buffer as *mut c_void, newsize) as *mut u8;
        if new_buffer.is_null() {
            if let Some(deallocate) = (*output_buffer).hooks.deallocate {
                deallocate((*output_buffer).buffer as *mut c_void);
            }
            (*output_buffer).length = 0;
            (*output_buffer).buffer = ptr::null_mut();
            return ptr::null_mut();
        }
    } else {
        let allocate = match (*output_buffer).hooks.allocate {
            Some(allocate) => allocate,
            None => return ptr::null_mut(),
        };
        new_buffer = allocate(newsize) as *mut u8;
        if new_buffer.is_null() {
            if let Some(deallocate) = (*output_buffer).hooks.deallocate {
                deallocate((*output_buffer).buffer as *mut c_void);
            }
            (*output_buffer).length = 0;
            (*output_buffer).buffer = ptr::null_mut();
            return ptr::null_mut();
        }
        ptr::copy_nonoverlapping((*output_buffer).buffer, new_buffer, (*output_buffer).offset + 1);
        if let Some(deallocate) = (*output_buffer).hooks.deallocate {
            deallocate((*output_buffer).buffer as *mut c_void);
        }
    }
    (*output_buffer).length = newsize;
    (*output_buffer).buffer = new_buffer;
    new_buffer.add((*output_buffer).offset)
}

/// Advance `offset` past the NUL-terminated text already written (`update_offset`).
unsafe fn update_offset(output_buffer: *mut printbuffer) {
    if output_buffer.is_null() || (*output_buffer).buffer.is_null() {
        return;
    }
    (*output_buffer).offset += c_strlen((*output_buffer).buffer.add((*output_buffer).offset));
}

/// Render a string pointer with escapes into the buffer (`print_string_ptr`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn print_string_ptr(input: *const u8, output_buffer: *mut printbuffer) -> cJSON_bool {
    if output_buffer.is_null() {
        return 0;
    }
    if input.is_null() {
        let output = ensure(output_buffer, 3);
        if output.is_null() {
            return 0;
        }
        *output = b'"';
        *output.add(1) = b'"';
        *output.add(2) = 0;
        return 1;
    }

    // Count how many bytes escaping will add.
    let mut escape_characters = 0usize;
    let mut input_pointer = input;
    while *input_pointer != 0 {
        match *input_pointer {
            b'"' | b'\\' | b'\x08' | b'\x0C' | b'\n' | b'\r' | b'\t' => escape_characters += 1,
            byte if byte < 32 => escape_characters += 5,
            _ => {}
        }
        input_pointer = input_pointer.add(1);
    }
    let output_length = input_pointer.offset_from(input) as usize + escape_characters;

    let output = ensure(output_buffer, output_length + 2);
    if output.is_null() {
        return 0;
    }

    if escape_characters == 0 {
        *output = b'"';
        ptr::copy_nonoverlapping(input, output.add(1), output_length);
        *output.add(output_length + 1) = b'"';
        *output.add(output_length + 2) = 0;
        return 1;
    }

    *output = b'"';
    let mut output_pointer = output.add(1);
    input_pointer = input;
    while *input_pointer != 0 {
        if *input_pointer > 31 && *input_pointer != b'"' && *input_pointer != b'\\' {
            *output_pointer = *input_pointer;
            output_pointer = output_pointer.add(1);
        } else {
            *output_pointer = b'\\';
            output_pointer = output_pointer.add(1);
            match *input_pointer {
                b'\\' => *output_pointer = b'\\',
                b'"' => *output_pointer = b'"',
                b'\x08' => *output_pointer = b'b',
                b'\x0C' => *output_pointer = b'f',
                b'\n' => *output_pointer = b'n',
                b'\r' => *output_pointer = b'r',
                b'\t' => *output_pointer = b't',
                byte => {
                    // escape as \u00xx
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    *output_pointer = b'u';
                    *output_pointer.add(1) = b'0';
                    *output_pointer.add(2) = b'0';
                    *output_pointer.add(3) = HEX[(byte >> 4) as usize];
                    *output_pointer.add(4) = HEX[(byte & 0x0F) as usize];
                    output_pointer = output_pointer.add(4);
                }
            }
            output_pointer = output_pointer.add(1);
        }
        input_pointer = input_pointer.add(1);
    }
    *output_pointer = b'"';
    *output_pointer.add(1) = 0;
    1
}

/// Render `item`'s string into the buffer (`print_string`).
unsafe fn print_string(item: *const cJSON, output_buffer: *mut printbuffer) -> cJSON_bool {
    print_string_ptr((*item).valuestring as *const u8, output_buffer)
}

/// Render a number into the buffer (`print_number`).
///
/// The digit formatting is delegated to the safe core's `format_number`, which
/// reproduces cJSON's `%1.15g`/`%1.17g` strategy exactly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn print_number(item: *const cJSON, output_buffer: *mut printbuffer) -> cJSON_bool {
    if output_buffer.is_null() {
        return 0;
    }
    let text = match crate::print_unformatted(&Value::Number((*item).valuedouble)) {
        Some(text) => text,
        None => return 0,
    };
    let length = text.len();
    let output = ensure(output_buffer, length + 1);
    if output.is_null() {
        return 0;
    }
    ptr::copy_nonoverlapping(text.as_ptr(), output, length);
    *output.add(length) = 0;
    (*output_buffer).offset += length;
    1
}

/// Render any value into the buffer (`print_value`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn print_value(item: *const cJSON, output_buffer: *mut printbuffer) -> cJSON_bool {
    if item.is_null() || output_buffer.is_null() {
        return 0;
    }
    let output: *mut u8;
    match (*item).ctype & 0xFF {
        CJSON_NULL => {
            output = ensure(output_buffer, 5);
            if output.is_null() {
                return 0;
            }
            ptr::copy_nonoverlapping(b"null\0".as_ptr(), output, 5);
            1
        }
        CJSON_FALSE => {
            output = ensure(output_buffer, 6);
            if output.is_null() {
                return 0;
            }
            ptr::copy_nonoverlapping(b"false\0".as_ptr(), output, 6);
            1
        }
        CJSON_TRUE => {
            output = ensure(output_buffer, 5);
            if output.is_null() {
                return 0;
            }
            ptr::copy_nonoverlapping(b"true\0".as_ptr(), output, 5);
            1
        }
        CJSON_NUMBER => print_number(item, output_buffer),
        CJSON_RAW => {
            if (*item).valuestring.is_null() {
                return 0;
            }
            let raw_length = c_strlen((*item).valuestring as *const u8) + 1;
            output = ensure(output_buffer, raw_length);
            if output.is_null() {
                return 0;
            }
            ptr::copy_nonoverlapping((*item).valuestring as *const u8, output, raw_length);
            1
        }
        CJSON_STRING => print_string(item, output_buffer),
        CJSON_ARRAY => print_array(item, output_buffer),
        CJSON_OBJECT => print_object(item, output_buffer),
        _ => 0,
    }
}

/// Render an array into the buffer (`print_array`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn print_array(item: *const cJSON, output_buffer: *mut printbuffer) -> cJSON_bool {
    if output_buffer.is_null() {
        return 0;
    }
    if (*output_buffer).depth >= CJSON_NESTING_LIMIT {
        return 0;
    }
    let output = ensure(output_buffer, 1);
    if output.is_null() {
        return 0;
    }
    *output = b'[';
    (*output_buffer).offset += 1;
    (*output_buffer).depth += 1;

    let mut current = (*item).child;
    while !current.is_null() {
        if print_value(current, output_buffer) == 0 {
            return 0;
        }
        update_offset(output_buffer);
        if !(*current).next.is_null() {
            let length = if (*output_buffer).format != 0 { 2 } else { 1 };
            let output = ensure(output_buffer, length + 1);
            if output.is_null() {
                return 0;
            }
            *output = b',';
            if (*output_buffer).format != 0 {
                *output.add(1) = b' ';
            }
            *output.add(length) = 0;
            (*output_buffer).offset += length;
        }
        current = (*current).next;
    }

    let output = ensure(output_buffer, 2);
    if output.is_null() {
        return 0;
    }
    *output = b']';
    *output.add(1) = 0;
    (*output_buffer).depth -= 1;
    1
}

/// Render an object into the buffer (`print_object`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn print_object(item: *const cJSON, output_buffer: *mut printbuffer) -> cJSON_bool {
    if output_buffer.is_null() {
        return 0;
    }
    if (*output_buffer).depth >= CJSON_NESTING_LIMIT {
        return 0;
    }
    let length = if (*output_buffer).format != 0 { 2 } else { 1 };
    let output = ensure(output_buffer, length + 1);
    if output.is_null() {
        return 0;
    }
    *output = b'{';
    (*output_buffer).depth += 1;
    if (*output_buffer).format != 0 {
        *output.add(1) = b'\n';
    }
    (*output_buffer).offset += length;

    let mut current = (*item).child;
    while !current.is_null() {
        if (*output_buffer).format != 0 {
            let output = ensure(output_buffer, (*output_buffer).depth);
            if output.is_null() {
                return 0;
            }
            for i in 0..(*output_buffer).depth {
                *output.add(i) = b'\t';
            }
            (*output_buffer).offset += (*output_buffer).depth;
        }
        if print_string_ptr((*current).string as *const u8, output_buffer) == 0 {
            return 0;
        }
        update_offset(output_buffer);

        let length = if (*output_buffer).format != 0 { 2 } else { 1 };
        let output = ensure(output_buffer, length);
        if output.is_null() {
            return 0;
        }
        *output = b':';
        if (*output_buffer).format != 0 {
            *output.add(1) = b'\t';
        }
        (*output_buffer).offset += length;

        if print_value(current, output_buffer) == 0 {
            return 0;
        }
        update_offset(output_buffer);

        // comma only between members; every member line ends with '\n' when
        // formatted (including the last one)
        let length = if (*output_buffer).format != 0 { 1 } else { 0 }
            + if !(*current).next.is_null() { 1 } else { 0 };
        let output = ensure(output_buffer, length + 1);
        if output.is_null() {
            return 0;
        }
        let mut write_at = 0usize;
        if !(*current).next.is_null() {
            *output = b',';
            write_at = 1;
        }
        if (*output_buffer).format != 0 {
            *output.add(write_at) = b'\n';
        }
        *output.add(length) = 0;
        (*output_buffer).offset += length;

        current = (*current).next;
    }

    if (*output_buffer).format != 0 {
        // the closing brace is preceded by (depth - 1) tabs
        let indent = (*output_buffer).depth - 1;
        let output = ensure(output_buffer, indent + 2);
        if output.is_null() {
            return 0;
        }
        for i in 0..indent {
            *output.add(i) = b'\t';
        }
        *output.add(indent) = b'}';
        *output.add(indent + 1) = 0;
    } else {
        let output = ensure(output_buffer, 2);
        if output.is_null() {
            return 0;
        }
        *output = b'}';
        *output.add(1) = 0;
    }
    (*output_buffer).depth -= 1;
    1
}

/// The static `print` from `cJSON.c`: render into a fresh buffer and shrink.
unsafe fn print_impl(item: *const cJSON, format: cJSON_bool) -> *mut c_char {
    const DEFAULT_BUFFER_SIZE: usize = 256;

    let mut buffer: printbuffer = core::mem::zeroed();
    buffer.buffer = hook_allocate(DEFAULT_BUFFER_SIZE) as *mut u8;
    buffer.length = DEFAULT_BUFFER_SIZE;
    buffer.format = format;
    buffer.hooks = global_hooks;
    if buffer.buffer.is_null() {
        return ptr::null_mut();
    }

    if print_value(item, &mut buffer) == 0 {
        hook_deallocate(buffer.buffer as *mut c_void);
        return ptr::null_mut();
    }
    update_offset(&mut buffer);

    let printed: *mut u8;
    if let Some(reallocate) = global_hooks.reallocate {
        printed = reallocate(buffer.buffer as *mut c_void, buffer.offset + 1) as *mut u8;
        if printed.is_null() {
            hook_deallocate(buffer.buffer as *mut c_void);
            return ptr::null_mut();
        }
    } else {
        printed = hook_allocate(buffer.offset + 1) as *mut u8;
        if printed.is_null() {
            hook_deallocate(buffer.buffer as *mut c_void);
            return ptr::null_mut();
        }
        let copy_len = core::cmp::min(buffer.length, buffer.offset + 1);
        ptr::copy_nonoverlapping(buffer.buffer, printed, copy_len);
        *printed.add(buffer.offset) = 0;
        hook_deallocate(buffer.buffer as *mut c_void);
    }
    printed as *mut c_char
}

// ---------------------------------------------------------------------------
// Value conversion helpers (for the cJSON_Utils delegation)
// ---------------------------------------------------------------------------

/// Deep-convert a node tree into the safe [`Value`] model.
unsafe fn node_to_value(node: *const cJSON) -> Value {
    if node.is_null() {
        return Value::Invalid;
    }
    match (*node).ctype & 0xFF {
        CJSON_NULL => Value::Null,
        CJSON_FALSE => Value::Bool(false),
        CJSON_TRUE => Value::Bool(true),
        CJSON_NUMBER => Value::Number((*node).valuedouble),
        CJSON_STRING => {
            if (*node).valuestring.is_null() {
                Value::String(String::new())
            } else {
                let bytes = core::slice::from_raw_parts((*node).valuestring as *const u8, c_strlen((*node).valuestring as *const u8));
                Value::String(String::from_utf8_lossy(bytes).into_owned())
            }
        }
        CJSON_RAW => {
            if (*node).valuestring.is_null() {
                Value::Raw(String::new())
            } else {
                let bytes = core::slice::from_raw_parts((*node).valuestring as *const u8, c_strlen((*node).valuestring as *const u8));
                Value::Raw(String::from_utf8_lossy(bytes).into_owned())
            }
        }
        CJSON_ARRAY => {
            let mut values = Vec::new();
            let mut child = (*node).child;
            while !child.is_null() {
                values.push(node_to_value(child));
                child = (*child).next;
            }
            Value::Array(values)
        }
        CJSON_OBJECT => {
            let mut members = Vec::new();
            let mut child = (*node).child;
            while !child.is_null() {
                let name = if (*child).string.is_null() {
                    String::new()
                } else {
                    let bytes = core::slice::from_raw_parts((*child).string as *const u8, c_strlen((*child).string as *const u8));
                    String::from_utf8_lossy(bytes).into_owned()
                };
                members.push(Member::new(name, node_to_value(child)));
                child = (*child).next;
            }
            Value::Object(members)
        }
        _ => Value::Invalid,
    }
}

/// Allocate a node tree that represents `value` through the active hooks.
unsafe fn value_to_node(value: &Value) -> *mut cJSON {
    let node = new_item();
    if node.is_null() {
        return ptr::null_mut();
    }
    match value {
        Value::Invalid => (*node).ctype = CJSON_INVALID,
        Value::Null => (*node).ctype = CJSON_NULL,
        Value::Bool(false) => (*node).ctype = CJSON_FALSE,
        Value::Bool(true) => {
            (*node).ctype = CJSON_TRUE;
            (*node).valueint = 1;
        }
        Value::Number(number) => {
            (*node).ctype = CJSON_NUMBER;
            (*node).valuedouble = *number;
            (*node).valueint = if *number >= 2_147_483_647.0 {
                c_int::MAX
            } else if *number <= -2_147_483_648.0 {
                c_int::MIN
            } else {
                *number as c_int
            };
        }
        Value::String(text) => {
            (*node).ctype = CJSON_STRING;
            (*node).valuestring = c_string(text) as *mut c_char;
        }
        Value::Raw(text) => {
            (*node).ctype = CJSON_RAW;
            (*node).valuestring = c_string(text) as *mut c_char;
        }
        Value::Array(values) => {
            (*node).ctype = CJSON_ARRAY;
            for value in values {
                let child = value_to_node(value);
                if !child.is_null() {
                    add_item_to_array_impl(node, child);
                }
            }
        }
        Value::Object(members) => {
            (*node).ctype = CJSON_OBJECT;
            for member in members {
                let child = value_to_node(&member.value);
                if !child.is_null() {
                    add_item_to_array_impl(node, child);
                    (*child).string = c_string(&member.name) as *mut c_char;
                }
            }
        }
    }
    node
}

/// Allocate a NUL-terminated copy of `text` through the active hooks.
unsafe fn c_string(text: &str) -> *mut u8 {
    let length = text.len();
    let copy = hook_allocate(length + 1) as *mut u8;
    if copy.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(text.as_ptr(), copy, length);
    *copy.add(length) = 0;
    copy
}

/// Replace `node`'s contents with a deep conversion of `value`, freeing the old
/// contents but keeping the node pointer (and its `string` name) alive.
unsafe fn overwrite_node(node: *mut cJSON, value: &Value) {
    // Free the current subtree and strings, but keep the node itself.
    delete_node((*node).child);
    (*node).child = ptr::null_mut();
    if ((*node).ctype & CJSON_IS_REFERENCE) == 0 && !(*node).valuestring.is_null() {
        hook_deallocate((*node).valuestring as *mut c_void);
    }
    (*node).valuestring = ptr::null_mut();

    // Preserve the member name while repopulating the payload.
    let name = (*node).string;
    let kind = (*node).ctype & !CJSON_STRING_IS_CONST;

    let replacement = value_to_node(value);
    let replacement_type = if replacement.is_null() { CJSON_INVALID } else { (*replacement).ctype };
    let replacement_valueint = if replacement.is_null() { 0 } else { (*replacement).valueint };
    let replacement_valuedouble = if replacement.is_null() { 0.0 } else { (*replacement).valuedouble };
    let replacement_valuestring = if replacement.is_null() { ptr::null_mut() } else { (*replacement).valuestring };
    let replacement_child = if replacement.is_null() { ptr::null_mut() } else { (*replacement).child };

    (*node).ctype = replacement_type | (kind & CJSON_STRING_IS_CONST);
    (*node).valueint = replacement_valueint;
    (*node).valuedouble = replacement_valuedouble;
    (*node).valuestring = replacement_valuestring;
    (*node).child = replacement_child;
    (*node).string = name;

    if !replacement.is_null() {
        // sever the temporary wrapper so we don't free its payload twice
        (*replacement).child = ptr::null_mut();
        (*replacement).valuestring = ptr::null_mut();
        hook_deallocate(replacement as *mut c_void);
    }
}

// ---------------------------------------------------------------------------
// Public cJSON API
// ---------------------------------------------------------------------------

/// `cJSON_Version`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Version() -> *const c_char {
    const VERSION: &[u8] = b"1.7.19\0";
    VERSION.as_ptr() as *const c_char
}

/// `cJSON_InitHooks`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_InitHooks(hooks: *mut cJSON_Hooks) {
    unsafe {
        if hooks.is_null() {
            global_hooks = internal_hooks {
                allocate: Some(malloc as unsafe extern "C" fn(usize) -> *mut c_void),
                deallocate: Some(free as unsafe extern "C" fn(*mut c_void)),
                reallocate: Some(realloc as unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void),
            };
            return;
        }
        let hooks = &*hooks;
        global_hooks.allocate = hooks.malloc_fn.or(Some(malloc as unsafe extern "C" fn(usize) -> *mut c_void));
        global_hooks.deallocate = hooks.free_fn.or(Some(free as unsafe extern "C" fn(*mut c_void)));
        global_hooks.reallocate = if hooks.malloc_fn.is_none() && hooks.free_fn.is_none() {
            Some(realloc as unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void)
        } else {
            None
        };
    }
}

/// `cJSON_Parse`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Parse(value: *const c_char) -> *mut cJSON {
    cJSON_ParseWithOpts(value, ptr::null_mut(), 0)
}

/// `cJSON_ParseWithLength`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ParseWithLength(value: *const c_char, buffer_length: usize) -> *mut cJSON {
    cJSON_ParseWithLengthOpts(value, buffer_length, ptr::null_mut(), 0)
}

/// `cJSON_ParseWithOpts`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ParseWithOpts(
    value: *const c_char,
    return_parse_end: *mut *const c_char,
    require_null_terminated: cJSON_bool,
) -> *mut cJSON {
    unsafe {
        let buffer_length = if value.is_null() { 0 } else { c_strlen(value as *const u8) + 1 };
        cJSON_ParseWithLengthOpts(value, buffer_length, return_parse_end, require_null_terminated)
    }
}

/// `cJSON_ParseWithLengthOpts`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ParseWithLengthOpts(
    value: *const c_char,
    buffer_length: usize,
    return_parse_end: *mut *const c_char,
    require_null_terminated: cJSON_bool,
) -> *mut cJSON {
    unsafe {
        // reset error position
        global_error = error { json: ptr::null(), position: 0 };

        let mut buffer: parse_buffer = core::mem::zeroed();
        if !value.is_null() {
            buffer.content = value as *const u8;
            buffer.length = buffer_length;
            buffer.hooks = global_hooks;
        }

        if value.is_null() || buffer_length == 0 {
            return finish_parse_error(value, return_parse_end, ptr::null_mut(), &buffer);
        }

        let item = new_item();
        if item.is_null() {
            return finish_parse_error(value, return_parse_end, item, &buffer);
        }

        skip_utf8_bom(&mut buffer);
        buffer_skip_whitespace(&mut buffer);

        if parse_value(item, &mut buffer) == 0 {
            return finish_parse_error(value, return_parse_end, item, &buffer);
        }

        if require_null_terminated != 0 {
            buffer_skip_whitespace(&mut buffer);
            if buffer.offset >= buffer.length || *buffer.content.add(buffer.offset) != 0 {
                return finish_parse_error(value, return_parse_end, item, &buffer);
            }
        }
        if !return_parse_end.is_null() {
            *return_parse_end = buffer.content.add(buffer.offset) as *const c_char;
        }
        item
    }
}

/// Shared failure path of the parse entry points: record the error position and
/// delete any partially built tree.
unsafe fn finish_parse_error(
    value: *const c_char,
    return_parse_end: *mut *const c_char,
    item: *mut cJSON,
    buffer: *const parse_buffer,
) -> *mut cJSON {
    if !item.is_null() {
        delete_node(item);
    }
    if !value.is_null() {
        let position = if (*buffer).offset < (*buffer).length {
            (*buffer).offset
        } else if (*buffer).length > 0 {
            (*buffer).length - 1
        } else {
            0
        };
        let local = error { json: value as *const u8, position };
        global_error = local;
        if !return_parse_end.is_null() {
            *return_parse_end = local.json.add(local.position) as *const c_char;
        }
    }
    ptr::null_mut()
}

/// `cJSON_Print`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Print(item: *const cJSON) -> *mut c_char {
    unsafe { print_impl(item, 1) }
}

/// `cJSON_PrintUnformatted`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_PrintUnformatted(item: *const cJSON) -> *mut c_char {
    unsafe { print_impl(item, 0) }
}

/// `cJSON_PrintBuffered`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_PrintBuffered(item: *const cJSON, prebuffer: c_int, format: cJSON_bool) -> *mut c_char {
    unsafe {
        if prebuffer < 0 {
            return ptr::null_mut();
        }
        let mut buffer: printbuffer = core::mem::zeroed();
        buffer.buffer = hook_allocate(prebuffer as usize) as *mut u8;
        if buffer.buffer.is_null() {
            return ptr::null_mut();
        }
        buffer.length = prebuffer as usize;
        buffer.format = format;
        buffer.hooks = global_hooks;
        if print_value(item, &mut buffer) == 0 {
            hook_deallocate(buffer.buffer as *mut c_void);
            return ptr::null_mut();
        }
        buffer.buffer as *mut c_char
    }
}

/// `cJSON_PrintPreallocated`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_PrintPreallocated(
    item: *const cJSON,
    buffer: *mut c_char,
    length: c_int,
    format: cJSON_bool,
) -> cJSON_bool {
    unsafe {
        if length < 0 || buffer.is_null() {
            return 0;
        }
        let mut p: printbuffer = core::mem::zeroed();
        p.buffer = buffer as *mut u8;
        p.length = length as usize;
        p.noalloc = 1;
        p.format = format;
        p.hooks = global_hooks;
        print_value(item, &mut p)
    }
}

/// `cJSON_Delete`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Delete(item: *mut cJSON) {
    unsafe { delete_node(item) }
}

/// `cJSON_GetArraySize`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetArraySize(array: *const cJSON) -> c_int {
    unsafe {
        if array.is_null() {
            return 0;
        }
        let mut size = 0usize;
        let mut child = (*array).child;
        while !child.is_null() {
            size += 1;
            child = (*child).next;
        }
        if size > c_int::MAX as usize {
            return -1;
        }
        size as c_int
    }
}

/// `cJSON_GetArrayItem`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetArrayItem(array: *const cJSON, index: c_int) -> *mut cJSON {
    unsafe {
        if index < 0 {
            return ptr::null_mut();
        }
        get_array_item(array, index as usize)
    }
}

/// `cJSON_GetObjectItem`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetObjectItem(object: *const cJSON, string: *const c_char) -> *mut cJSON {
    unsafe { get_object_item(object, string, 0) }
}

/// `cJSON_GetObjectItemCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetObjectItemCaseSensitive(object: *const cJSON, string: *const c_char) -> *mut cJSON {
    unsafe { get_object_item(object, string, 1) }
}

/// `cJSON_HasObjectItem`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_HasObjectItem(object: *const cJSON, string: *const c_char) -> cJSON_bool {
    if cJSON_GetObjectItem(object, string).is_null() {
        0
    } else {
        1
    }
}

/// `cJSON_GetErrorPtr`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetErrorPtr() -> *const c_char {
    unsafe { global_error.json.add(global_error.position) as *const c_char }
}

/// `cJSON_GetStringValue`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetStringValue(item: *const cJSON) -> *mut c_char {
    unsafe {
        if item.is_null() || ((*item).ctype & 0xFF) != CJSON_STRING {
            return ptr::null_mut();
        }
        (*item).valuestring
    }
}

/// `cJSON_GetNumberValue`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_GetNumberValue(item: *const cJSON) -> c_double {
    unsafe {
        if item.is_null() || ((*item).ctype & 0xFF) != CJSON_NUMBER {
            return f64::NAN;
        }
        (*item).valuedouble
    }
}

macro_rules! is_kind_fn {
    ($name:ident, $kind:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(item: *const cJSON) -> cJSON_bool {
            unsafe {
                if item.is_null() {
                    return 0;
                }
                if ((*item).ctype & 0xFF) == $kind {
                    1
                } else {
                    0
                }
            }
        }
    };
}

is_kind_fn!(cJSON_IsInvalid, CJSON_INVALID);
is_kind_fn!(cJSON_IsFalse, CJSON_FALSE);
is_kind_fn!(cJSON_IsTrue, CJSON_TRUE);
is_kind_fn!(cJSON_IsNull, CJSON_NULL);
is_kind_fn!(cJSON_IsNumber, CJSON_NUMBER);
is_kind_fn!(cJSON_IsString, CJSON_STRING);
is_kind_fn!(cJSON_IsArray, CJSON_ARRAY);
is_kind_fn!(cJSON_IsObject, CJSON_OBJECT);
is_kind_fn!(cJSON_IsRaw, CJSON_RAW);

/// `cJSON_IsBool` (true/false)
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_IsBool(item: *const cJSON) -> cJSON_bool {
    unsafe {
        if item.is_null() {
            return 0;
        }
        let kind = (*item).ctype & 0xFF;
        if kind == CJSON_TRUE || kind == CJSON_FALSE {
            1
        } else {
            0
        }
    }
}

unsafe fn create_number_node(number: f64) -> *mut cJSON {
    let item = new_item();
    if item.is_null() {
        return ptr::null_mut();
    }
    (*item).ctype = CJSON_NUMBER;
    (*item).valuedouble = number;
    (*item).valueint = if number >= 2_147_483_647.0 {
        c_int::MAX
    } else if number <= -2_147_483_648.0 {
        c_int::MIN
    } else {
        number as c_int
    };
    item
}

/// `cJSON_CreateNull`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateNull() -> *mut cJSON {
    unsafe {
        let item = new_item();
        if !item.is_null() {
            (*item).ctype = CJSON_NULL;
        }
        item
    }
}

/// `cJSON_CreateTrue`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateTrue() -> *mut cJSON {
    unsafe {
        let item = new_item();
        if !item.is_null() {
            (*item).ctype = CJSON_TRUE;
            (*item).valueint = 1;
        }
        item
    }
}

/// `cJSON_CreateFalse`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateFalse() -> *mut cJSON {
    unsafe {
        let item = new_item();
        if !item.is_null() {
            (*item).ctype = CJSON_FALSE;
        }
        item
    }
}

/// `cJSON_CreateBool`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateBool(boolean: cJSON_bool) -> *mut cJSON {
    unsafe {
        let item = new_item();
        if !item.is_null() {
            (*item).ctype = if boolean != 0 { CJSON_TRUE } else { CJSON_FALSE };
            (*item).valueint = if boolean != 0 { 1 } else { 0 };
        }
        item
    }
}

/// `cJSON_CreateNumber`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateNumber(number: c_double) -> *mut cJSON {
    unsafe { create_number_node(number) }
}

/// `cJSON_CreateString` (a NULL string yields NULL, matching cJSON.c)
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateString(string: *const c_char) -> *mut cJSON {
    unsafe {
        let item = new_item();
        if item.is_null() {
            return ptr::null_mut();
        }
        (*item).ctype = CJSON_STRING;
        (*item).valuestring = c_json_strdup_impl(string as *const u8, ptr::null()) as *mut c_char;
        if (*item).valuestring.is_null() {
            delete_node(item);
            return ptr::null_mut();
        }
        item
    }
}

/// `cJSON_CreateRaw` (a NULL raw yields NULL, matching cJSON.c)
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateRaw(raw: *const c_char) -> *mut cJSON {
    unsafe {
        let item = new_item();
        if item.is_null() {
            return ptr::null_mut();
        }
        (*item).ctype = CJSON_RAW;
        (*item).valuestring = c_json_strdup_impl(raw as *const u8, ptr::null()) as *mut c_char;
        if (*item).valuestring.is_null() {
            delete_node(item);
            return ptr::null_mut();
        }
        item
    }
}

/// `cJSON_CreateArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateArray() -> *mut cJSON {
    unsafe {
        let item = new_item();
        if !item.is_null() {
            (*item).ctype = CJSON_ARRAY;
        }
        item
    }
}

/// `cJSON_CreateObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateObject() -> *mut cJSON {
    unsafe {
        let item = new_item();
        if !item.is_null() {
            (*item).ctype = CJSON_OBJECT;
        }
        item
    }
}

/// `cJSON_CreateStringReference`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateStringReference(string: *const c_char) -> *mut cJSON {
    unsafe {
        let item = new_item();
        if item.is_null() {
            return ptr::null_mut();
        }
        (*item).ctype = CJSON_STRING | CJSON_IS_REFERENCE;
        (*item).valuestring = string as *mut c_char;
        item
    }
}

/// `cJSON_CreateObjectReference`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateObjectReference(child: *const cJSON) -> *mut cJSON {
    unsafe {
        let item = new_item();
        if item.is_null() {
            return ptr::null_mut();
        }
        (*item).ctype = CJSON_OBJECT | CJSON_IS_REFERENCE;
        (*item).child = child as *mut cJSON;
        item
    }
}

/// `cJSON_CreateArrayReference`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateArrayReference(child: *const cJSON) -> *mut cJSON {
    unsafe {
        let item = new_item();
        if item.is_null() {
            return ptr::null_mut();
        }
        (*item).ctype = CJSON_ARRAY | CJSON_IS_REFERENCE;
        (*item).child = child as *mut cJSON;
        item
    }
}

unsafe fn create_int_array_impl(numbers: *const c_int, count: c_int) -> *mut cJSON {
    if count < 0 || numbers.is_null() {
        return ptr::null_mut();
    }
    let array = new_item();
    if array.is_null() {
        return ptr::null_mut();
    }
    (*array).ctype = CJSON_ARRAY;
    for i in 0..count as usize {
        let number = *numbers.add(i as usize);
        let node = create_number_node(number as f64);
        if !node.is_null() {
            add_item_to_array_impl(array, node);
        }
    }
    array
}

unsafe fn create_float_array_impl(numbers: *const f32, count: c_int) -> *mut cJSON {
    if count < 0 || numbers.is_null() {
        return ptr::null_mut();
    }
    let array = new_item();
    if array.is_null() {
        return ptr::null_mut();
    }
    (*array).ctype = CJSON_ARRAY;
    for i in 0..count as usize {
        let number = *numbers.add(i as usize);
        let node = create_number_node(number as f64);
        if !node.is_null() {
            add_item_to_array_impl(array, node);
        }
    }
    array
}

unsafe fn create_double_array_impl(numbers: *const f64, count: c_int) -> *mut cJSON {
    if count < 0 || numbers.is_null() {
        return ptr::null_mut();
    }
    let array = new_item();
    if array.is_null() {
        return ptr::null_mut();
    }
    (*array).ctype = CJSON_ARRAY;
    for i in 0..count as usize {
        let number = *numbers.add(i as usize);
        let node = create_number_node(number);
        if !node.is_null() {
            add_item_to_array_impl(array, node);
        }
    }
    array
}

unsafe fn create_string_array_impl(strings: *const *const c_char, count: c_int) -> *mut cJSON {
    if count < 0 || strings.is_null() {
        return ptr::null_mut();
    }
    let array = new_item();
    if array.is_null() {
        return ptr::null_mut();
    }
    (*array).ctype = CJSON_ARRAY;
    for i in 0..count as usize {
        let string = *strings.add(i as usize);
        let node = new_item();
        if node.is_null() {
            continue;
        }
        (*node).ctype = CJSON_STRING;
        if !string.is_null() {
            (*node).valuestring = c_json_strdup_impl(string as *const u8, ptr::null()) as *mut c_char;
        }
        add_item_to_array_impl(array, node);
    }
    array
}

/// `cJSON_CreateIntArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateIntArray(numbers: *const c_int, count: c_int) -> *mut cJSON {
    unsafe { create_int_array_impl(numbers, count) }
}

/// `cJSON_CreateFloatArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateFloatArray(numbers: *const f32, count: c_int) -> *mut cJSON {
    unsafe { create_float_array_impl(numbers, count) }
}

/// `cJSON_CreateDoubleArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateDoubleArray(numbers: *const f64, count: c_int) -> *mut cJSON {
    unsafe { create_double_array_impl(numbers, count) }
}

/// `cJSON_CreateStringArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_CreateStringArray(strings: *const *const c_char, count: c_int) -> *mut cJSON {
    unsafe { create_string_array_impl(strings, count) }
}

/// `cJSON_AddItemToArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddItemToArray(array: *mut cJSON, item: *mut cJSON) -> cJSON_bool {
    unsafe { add_item_to_array_impl(array, item) }
}

/// `cJSON_AddItemToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddItemToObject(object: *mut cJSON, string: *const c_char, item: *mut cJSON) -> cJSON_bool {
    unsafe { add_item_to_object_impl(object, string, item, ptr::null(), 0) }
}

/// `cJSON_AddItemToObjectCS` (const-key variant)
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddItemToObjectCS(object: *mut cJSON, string: *const c_char, item: *mut cJSON) -> cJSON_bool {
    unsafe { add_item_to_object_impl(object, string, item, ptr::null(), 1) }
}

/// `cJSON_AddItemReferenceToArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddItemReferenceToArray(array: *mut cJSON, item: *const cJSON) -> cJSON_bool {
    unsafe {
        if array.is_null() {
            return 0;
        }
        add_item_to_array_impl(array, create_reference(item))
    }
}

/// `cJSON_AddItemReferenceToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddItemReferenceToObject(
    object: *mut cJSON,
    string: *const c_char,
    item: *const cJSON,
) -> cJSON_bool {
    unsafe {
        if object.is_null() {
            return 0;
        }
        add_item_to_object_impl(object, string, create_reference(item), ptr::null(), 0)
    }
}

/// Shared helper for the `cJSON_Add*ToObject` convenience functions.
unsafe fn add_child_to_object(object: *mut cJSON, name: *const c_char, child: *mut cJSON) -> *mut cJSON {
    if add_item_to_object_impl(object, name, child, ptr::null(), 0) != 0 {
        return child;
    }
    delete_node(child);
    ptr::null_mut()
}

/// `cJSON_AddNullToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddNullToObject(object: *mut cJSON, name: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateNull()) }
}

/// `cJSON_AddTrueToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddTrueToObject(object: *mut cJSON, name: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateTrue()) }
}

/// `cJSON_AddFalseToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddFalseToObject(object: *mut cJSON, name: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateFalse()) }
}

/// `cJSON_AddBoolToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddBoolToObject(object: *mut cJSON, name: *const c_char, boolean: cJSON_bool) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateBool(boolean)) }
}

/// `cJSON_AddNumberToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddNumberToObject(object: *mut cJSON, name: *const c_char, number: c_double) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, create_number_node(number)) }
}

/// `cJSON_AddStringToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddStringToObject(object: *mut cJSON, name: *const c_char, string: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateString(string)) }
}

/// `cJSON_AddRawToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddRawToObject(object: *mut cJSON, name: *const c_char, raw: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateRaw(raw)) }
}

/// `cJSON_AddObjectToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddObjectToObject(object: *mut cJSON, name: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateObject()) }
}

/// `cJSON_AddArrayToObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_AddArrayToObject(object: *mut cJSON, name: *const c_char) -> *mut cJSON {
    unsafe { add_child_to_object(object, name, cJSON_CreateArray()) }
}

/// `cJSON_DetachItemViaPointer`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DetachItemViaPointer(parent: *mut cJSON, item: *mut cJSON) -> *mut cJSON {
    unsafe {
        if parent.is_null() || item.is_null() || (item != (*parent).child && (*item).prev.is_null()) {
            return ptr::null_mut();
        }

        if item != (*parent).child {
            (*(*item).prev).next = (*item).next;
        }
        if !(*item).next.is_null() {
            (*(*item).next).prev = (*item).prev;
        }

        if item == (*parent).child {
            (*parent).child = (*item).next;
        } else if (*item).next.is_null() {
            // last element: keep head->prev pointing at the new tail
            (*(*parent).child).prev = (*item).prev;
        }

        (*item).prev = ptr::null_mut();
        (*item).next = ptr::null_mut();
        item
    }
}

/// `cJSON_DetachItemFromArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DetachItemFromArray(array: *mut cJSON, which: c_int) -> *mut cJSON {
    unsafe {
        if which < 0 {
            return ptr::null_mut();
        }
        cJSON_DetachItemViaPointer(array, get_array_item(array, which as usize))
    }
}

/// `cJSON_DeleteItemFromArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DeleteItemFromArray(array: *mut cJSON, which: c_int) {
    unsafe {
        let item = cJSON_DetachItemFromArray(array, which);
        delete_node(item);
    }
}

/// `cJSON_DetachItemFromObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DetachItemFromObject(object: *mut cJSON, string: *const c_char) -> *mut cJSON {
    cJSON_DetachItemViaPointer(object, cJSON_GetObjectItem(object, string))
}

/// `cJSON_DetachItemFromObjectCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DetachItemFromObjectCaseSensitive(object: *mut cJSON, string: *const c_char) -> *mut cJSON {
    cJSON_DetachItemViaPointer(object, cJSON_GetObjectItemCaseSensitive(object, string))
}

/// `cJSON_DeleteItemFromObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DeleteItemFromObject(object: *mut cJSON, string: *const c_char) {
    unsafe {
        let item = cJSON_DetachItemFromObject(object, string);
        delete_node(item);
    }
}

/// `cJSON_DeleteItemFromObjectCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_DeleteItemFromObjectCaseSensitive(object: *mut cJSON, string: *const c_char) {
    unsafe {
        let item = cJSON_DetachItemFromObjectCaseSensitive(object, string);
        delete_node(item);
    }
}

/// `cJSON_InsertItemInArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_InsertItemInArray(array: *mut cJSON, which: c_int, newitem: *mut cJSON) -> cJSON_bool {
    unsafe {
        if which < 0 || newitem.is_null() {
            return 0;
        }
        let after_inserted = get_array_item(array, which as usize);
        if after_inserted.is_null() {
            return add_item_to_array_impl(array, newitem);
        }
        if after_inserted != (*array).child && (*after_inserted).prev.is_null() {
            return 0; // corrupted array item
        }
        (*newitem).next = after_inserted;
        (*newitem).prev = (*after_inserted).prev;
        (*after_inserted).prev = newitem;
        if after_inserted == (*array).child {
            (*array).child = newitem;
        } else {
            (*(*newitem).prev).next = newitem;
        }
        1
    }
}

/// `cJSON_ReplaceItemViaPointer`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ReplaceItemViaPointer(parent: *mut cJSON, item: *mut cJSON, replacement: *mut cJSON) -> cJSON_bool {
    unsafe {
        if parent.is_null() || (*parent).child.is_null() || replacement.is_null() || item.is_null() {
            return 0;
        }
        if replacement == item {
            return 1;
        }

        (*replacement).next = (*item).next;
        (*replacement).prev = (*item).prev;

        if !(*replacement).next.is_null() {
            (*(*replacement).next).prev = replacement;
        }
        if (*parent).child == item {
            if (*(*parent).child).prev == (*parent).child {
                // the replaced item was the only element
                (*parent).child = replacement;
                (*replacement).prev = replacement;
            } else {
                (*parent).child = replacement;
            }
        } else {
            if !(*replacement).prev.is_null() {
                (*(*replacement).prev).next = replacement;
            }
            if (*replacement).next.is_null() {
                // replaced the tail: keep head->prev == tail
                (*(*parent).child).prev = replacement;
            }
        }

        (*item).next = ptr::null_mut();
        (*item).prev = ptr::null_mut();
        delete_node(item);
        1
    }
}

/// `cJSON_ReplaceItemInArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ReplaceItemInArray(array: *mut cJSON, which: c_int, newitem: *mut cJSON) -> cJSON_bool {
    unsafe {
        if which < 0 {
            return 0;
        }
        cJSON_ReplaceItemViaPointer(array, get_array_item(array, which as usize), newitem)
    }
}

/// `cJSON_ReplaceItemInObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ReplaceItemInObject(object: *mut cJSON, string: *const c_char, newitem: *mut cJSON) -> cJSON_bool {
    unsafe { replace_item_in_object_impl(object, string, newitem, 0) }
}

/// `cJSON_ReplaceItemInObjectCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_ReplaceItemInObjectCaseSensitive(
    object: *mut cJSON,
    string: *const c_char,
    newitem: *mut cJSON,
) -> cJSON_bool {
    unsafe { replace_item_in_object_impl(object, string, newitem, 1) }
}

unsafe fn replace_item_in_object_impl(
    object: *mut cJSON,
    string: *const c_char,
    replacement: *mut cJSON,
    case_sensitive: cJSON_bool,
) -> cJSON_bool {
    if replacement.is_null() || string.is_null() {
        return 0;
    }
    if ((*replacement).ctype & CJSON_STRING_IS_CONST) == 0 && !(*replacement).string.is_null() {
        hook_deallocate((*replacement).string as *mut c_void);
    }
    (*replacement).string = c_json_strdup_impl(string as *const u8, ptr::null()) as *mut c_char;
    if (*replacement).string.is_null() {
        return 0;
    }
    (*replacement).ctype &= !CJSON_STRING_IS_CONST;
    cJSON_ReplaceItemViaPointer(object, get_object_item(object, string, case_sensitive), replacement)
}

/// `cJSON_Duplicate`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Duplicate(item: *const cJSON, recurse: cJSON_bool) -> *mut cJSON {
    unsafe { duplicate_rec(item, 0, recurse != 0) }
}

unsafe fn duplicate_rec(item: *const cJSON, depth: usize, recurse: bool) -> *mut cJSON {
    if item.is_null() {
        return ptr::null_mut();
    }
    let newitem = new_item();
    if newitem.is_null() {
        return ptr::null_mut();
    }

    (*newitem).ctype = (*item).ctype & !CJSON_IS_REFERENCE;
    (*newitem).valueint = (*item).valueint;
    (*newitem).valuedouble = (*item).valuedouble;
    if !(*item).valuestring.is_null() {
        (*newitem).valuestring = c_json_strdup_impl((*item).valuestring as *const u8, ptr::null()) as *mut c_char;
        if (*newitem).valuestring.is_null() {
            delete_node(newitem);
            return ptr::null_mut();
        }
    }
    if !(*item).string.is_null() {
        (*newitem).string = if ((*item).ctype & CJSON_STRING_IS_CONST) != 0 {
            (*item).string
        } else {
            c_json_strdup_impl((*item).string as *const u8, ptr::null()) as *mut c_char
        };
        if (*newitem).string.is_null() {
            delete_node(newitem);
            return ptr::null_mut();
        }
    }

    if !recurse {
        return newitem;
    }

    let mut child = (*item).child;
    let mut tail: *mut cJSON = ptr::null_mut();
    while !child.is_null() {
        if depth >= CJSON_CIRCULAR_LIMIT {
            delete_node(newitem);
            return ptr::null_mut();
        }
        let newchild = duplicate_rec(child, depth + 1, true);
        if newchild.is_null() {
            delete_node(newitem);
            return ptr::null_mut();
        }
        if tail.is_null() {
            (*newitem).child = newchild;
        } else {
            (*tail).next = newchild;
            (*newchild).prev = tail;
        }
        tail = newchild;
        child = (*child).next;
    }

    newitem
}

/// `cJSON_Compare`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Compare(a: *const cJSON, b: *const cJSON, case_sensitive: cJSON_bool) -> cJSON_bool {
    unsafe {
        if a.is_null() || b.is_null() {
            return 0;
        }
        compare_node(a, b, case_sensitive != 0)
    }
}

unsafe fn compare_node(a: *const cJSON, b: *const cJSON, case_sensitive: bool) -> cJSON_bool {
    if ((*a).ctype & 0xFF) != ((*b).ctype & 0xFF) {
        return 0;
    }
    let kind = (*a).ctype & 0xFF;
    match kind {
        CJSON_NULL | CJSON_FALSE | CJSON_TRUE | CJSON_NUMBER | CJSON_STRING | CJSON_RAW | CJSON_ARRAY
        | CJSON_OBJECT => {}
        // invalid (and any unknown type) is never equal, even to itself
        _ => return 0,
    }
    // identical objects are equal (only reached for valid types)
    if a == b {
        return 1;
    }
    match kind {
        CJSON_NULL | CJSON_FALSE | CJSON_TRUE => 1,
        CJSON_NUMBER => compare_double((*a).valuedouble, (*b).valuedouble),
        CJSON_STRING | CJSON_RAW => {
            if (*a).valuestring.is_null() || (*b).valuestring.is_null() {
                return 0;
            }
            if c_strcmp((*a).valuestring as *const u8, (*b).valuestring as *const u8) == 0 {
                1
            } else {
                0
            }
        }
        CJSON_ARRAY => {
            let mut a_element = (*a).child;
            let mut b_element = (*b).child;
            while !a_element.is_null() && !b_element.is_null() {
                if compare_node(a_element, b_element, case_sensitive) == 0 {
                    return 0;
                }
                a_element = (*a_element).next;
                b_element = (*b_element).next;
            }
            // one of the arrays is longer than the other
            if a_element == b_element {
                1
            } else {
                0
            }
        }
        CJSON_OBJECT => {
            // a must be a subset of b, and b must be a subset of a
            let mut a_element = (*a).child;
            while !a_element.is_null() {
                let b_element = get_object_item(b, (*a_element).string, if case_sensitive { 1 } else { 0 });
                if b_element.is_null() {
                    return 0;
                }
                if compare_node(a_element, b_element, case_sensitive) == 0 {
                    return 0;
                }
                a_element = (*a_element).next;
            }
            let mut b_element = (*b).child;
            while !b_element.is_null() {
                let a_element = get_object_item(a, (*b_element).string, if case_sensitive { 1 } else { 0 });
                if a_element.is_null() {
                    return 0;
                }
                if compare_node(b_element, a_element, case_sensitive) == 0 {
                    return 0;
                }
                b_element = (*b_element).next;
            }
            1
        }
        _ => 0,
    }
}

/// The `compare_double` helper (exported for the white-box tests).
#[unsafe(no_mangle)]
pub extern "C" fn compare_double(a: c_double, b: c_double) -> cJSON_bool {
    let max_val = if a.abs() > b.abs() { a.abs() } else { b.abs() };
    if (a - b).abs() <= max_val * f64::EPSILON {
        1
    } else {
        0
    }
}

/// `cJSON_SetNumberHelper`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_SetNumberHelper(object: *mut cJSON, number: c_double) -> c_double {
    unsafe {
        if object.is_null() {
            return f64::NAN;
        }
        (*object).valueint = if number >= 2_147_483_647.0 {
            c_int::MAX
        } else if number <= -2_147_483_648.0 {
            c_int::MIN
        } else {
            number as c_int
        };
        (*object).valuedouble = number;
        number
    }
}

/// `cJSON_SetValuestring`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_SetValuestring(object: *mut cJSON, valuestring: *const c_char) -> *mut c_char {
    unsafe {
        if object.is_null()
            || ((*object).ctype & 0xFF) != CJSON_STRING
            || ((*object).ctype & CJSON_IS_REFERENCE) != 0
            || (*object).valuestring.is_null()
            || valuestring.is_null()
        {
            return ptr::null_mut();
        }

        let v1_len = c_strlen(valuestring as *const u8);
        let v2_len = c_strlen((*object).valuestring as *const u8);

        if v1_len <= v2_len {
            // strcpy-style in-place write, refusing overlapping ranges
            let value_start = valuestring as usize;
            let value_end = value_start + v1_len;
            let own_start = (*object).valuestring as usize;
            let own_end = own_start + v2_len;
            let disjoint = value_end < own_start || own_end < value_start;
            if !disjoint {
                return ptr::null_mut();
            }
            ptr::copy_nonoverlapping(valuestring as *const u8, (*object).valuestring as *mut u8, v1_len);
            *((*object).valuestring as *mut u8).add(v1_len) = 0;
            (*object).valuestring
        } else {
            let copy = c_json_strdup_impl(valuestring as *const u8, ptr::null()) as *mut c_char;
            if copy.is_null() {
                return ptr::null_mut();
            }
            if !(*object).valuestring.is_null() {
                hook_deallocate((*object).valuestring as *mut c_void);
            }
            (*object).valuestring = copy;
            copy
        }
    }
}

/// `cJSON_Minify` (in-place, mutating the caller's buffer).
///
/// Faithful port of `cJSON.c`'s `minify_string` / `skip_oneline_comment` /
/// `skip_multiline_comment` helpers.
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_Minify(json: *mut c_char) {
    unsafe {
        if json.is_null() {
            return;
        }
        let mut input = json as *mut u8;
        let mut into = json as *mut u8;
        while *input != 0 {
            match *input {
                b' ' | b'\t' | b'\r' | b'\n' => input = input.add(1),
                b'/' if *input.add(1) == b'/' => {
                    // skip_oneline_comment
                    input = input.add(2);
                    while *input != 0 {
                        if *input == b'\n' {
                            input = input.add(1);
                            break;
                        }
                        input = input.add(1);
                    }
                }
                b'/' if *input.add(1) == b'*' => {
                    // skip_multiline_comment
                    input = input.add(2);
                    while *input != 0 {
                        if *input == b'*' && *input.add(1) == b'/' {
                            input = input.add(2);
                            break;
                        }
                        input = input.add(1);
                    }
                }
                b'"' => {
                    // minify_string: copy a JSON string verbatim
                    *into = *input;
                    into = into.add(1);
                    input = input.add(1);
                    loop {
                        *into = *input;
                        if *input == b'"' {
                            into = into.add(1);
                            input = input.add(1);
                            break;
                        } else if *input == b'\\' && *input.add(1) == b'"' {
                            *into.add(1) = b'"';
                            into = into.add(1);
                            input = input.add(1);
                            // the next iteration copies the byte after the
                            // escaped quote and advances both again
                            into = into.add(1);
                            input = input.add(1);
                        } else if *input == 0 {
                            break; // unterminated string: stop at NUL
                        } else {
                            into = into.add(1);
                            input = input.add(1);
                        }
                    }
                }
                _ => {
                    *into = *input;
                    into = into.add(1);
                    input = input.add(1);
                }
            }
        }
        *into = 0;
    }
}

/// `cJSON_malloc`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_malloc(size: usize) -> *mut c_void {
    unsafe { hook_allocate(size) }
}

/// `cJSON_free`
#[unsafe(no_mangle)]
pub extern "C" fn cJSON_free(object: *mut c_void) {
    unsafe { hook_deallocate(object) }
}

/// `cJSON_strdup` (internal, exported for the white-box tests)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cJSON_strdup(string: *const u8, hooks: *const internal_hooks) -> *mut u8 {
    c_json_strdup_impl(string, hooks)
}

/// `add_item_to_array` (internal, exported for the white-box tests)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn add_item_to_array(array: *mut cJSON, item: *mut cJSON) -> cJSON_bool {
    add_item_to_array_impl(array, item)
}

// ---------------------------------------------------------------------------
// cJSON_Utils
// ---------------------------------------------------------------------------

/// `cJSONUtils_GetPointer`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_GetPointer(object: *const cJSON, pointer: *const c_char) -> *mut cJSON {
    unsafe { get_pointer_impl(object, pointer, 0) }
}

/// `cJSONUtils_GetPointerCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_GetPointerCaseSensitive(object: *const cJSON, pointer: *const c_char) -> *mut cJSON {
    unsafe { get_pointer_impl(object, pointer, 1) }
}

unsafe fn get_pointer_impl(object: *const cJSON, pointer: *const c_char, case_sensitive: cJSON_bool) -> *mut cJSON {
    if object.is_null() || pointer.is_null() {
        return ptr::null_mut();
    }
    let len = c_strlen(pointer as *const u8);
    let pointer_bytes = core::slice::from_raw_parts(pointer as *const u8, len);
    if pointer_bytes.is_empty() {
        // an empty pointer refers to the whole document
        return object as *mut cJSON;
    }
    if pointer_bytes[0] != b'/' {
        return ptr::null_mut();
    }

    let mut current = object as *mut cJSON;
    for segment in pointer_bytes.split(|&byte| byte == b'/').skip(1) {
        if current.is_null() {
            return ptr::null_mut();
        }
        current = descend(current, segment, case_sensitive != 0);
        if current.is_null() {
            return ptr::null_mut();
        }
    }
    current
}

unsafe fn descend(current: *mut cJSON, segment: &[u8], case_sensitive: bool) -> *mut cJSON {
    if current.is_null() {
        return ptr::null_mut();
    }
    match (*current).ctype & 0xFF {
        CJSON_OBJECT => {
            let mut child = (*current).child;
            while !child.is_null() {
                if !(*child).string.is_null() {
                    let name = core::slice::from_raw_parts((*child).string as *const u8, c_strlen((*child).string as *const u8));
                    if pointer_segment_matches(name, segment, case_sensitive) {
                        return child;
                    }
                }
                child = (*child).next;
            }
            ptr::null_mut()
        }
        CJSON_ARRAY => {
            let index_text = core::str::from_utf8(segment).ok();
            let index = index_text.and_then(decode_array_index_utils);
            match index {
                Some(index) => get_array_item(current, index),
                None => ptr::null_mut(),
            }
        }
        _ => ptr::null_mut(),
    }
}

/// Compare a member name against a JSON-pointer segment, decoding `~0`/`~1`.
fn pointer_segment_matches(name: &[u8], segment: &[u8], case_sensitive: bool) -> bool {
    let mut name_iter = name.iter();
    let mut seg_iter = segment.iter();
    loop {
        match (name_iter.next(), seg_iter.next()) {
            (None, None) => return true,
            (Some(&n), Some(&s)) if s == b'~' => match seg_iter.next() {
                Some(b'0') if n == b'~' => {}
                Some(b'1') if n == b'/' => {}
                _ => return false,
            },
            (Some(&n), Some(&s)) => {
                if case_sensitive {
                    if n != s {
                        return false;
                    }
                } else if to_lower(n) != to_lower(s) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

fn decode_array_index_utils(token: &str) -> Option<usize> {
    if token.is_empty() || (token.starts_with('0') && token.len() > 1) {
        return None;
    }
    if !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// `cJSONUtils_FindPointerFromObjectTo`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_FindPointerFromObjectTo(object: *const cJSON, target: *const cJSON) -> *mut c_char {
    unsafe { find_pointer_rec(object, target) }
}

/// Recursive depth-first search for `target` that builds the pointer string
/// from the leaf up: the base case contributes "", and each level prepends
/// `/<index>` (arrays) or `/<encoded name>` (objects).
unsafe fn find_pointer_rec(object: *const cJSON, target: *const cJSON) -> *mut c_char {
    if object.is_null() || target.is_null() {
        return ptr::null_mut();
    }
    if object == target {
        return c_string("") as *mut c_char;
    }

    let mut child_index = 0usize;
    let mut current_child = (*object).child;
    while !current_child.is_null() {
        let target_pointer = find_pointer_rec(current_child, target);
        if !target_pointer.is_null() {
            // `current_child` is on the path to `target`; build this level's prefix.
            let target_text = String::from_utf8_lossy(core::slice::from_raw_parts(
                target_pointer as *const u8,
                c_strlen(target_pointer as *const u8),
            ))
            .into_owned();
            hook_deallocate(target_pointer as *mut c_void);

            let full = match (*object).ctype & 0xFF {
                CJSON_ARRAY => format!("/{child_index}{target_text}"),
                CJSON_OBJECT => {
                    let name = if (*current_child).string.is_null() {
                        String::new()
                    } else {
                        String::from_utf8_lossy(core::slice::from_raw_parts(
                            (*current_child).string as *const u8,
                            c_strlen((*current_child).string as *const u8),
                        ))
                        .into_owned()
                    };
                    format!("/{}{target_text}", encode_pointer_fragment(&name))
                }
                _ => {
                    // reached a leaf: the recursive call cannot have found the
                    // target under a non-container
                    return ptr::null_mut();
                }
            };
            return c_string(&full) as *mut c_char;
        }
        child_index += 1;
        current_child = (*current_child).next;
    }
    ptr::null_mut()
}

/// Escape `~` and `/` in a member name using `~0`/`~1` pointer encodings.
fn encode_pointer_fragment(fragment: &str) -> String {
    let mut encoded = String::with_capacity(fragment.len());
    for byte in fragment.bytes() {
        match byte {
            b'~' => encoded.push_str("~0"),
            b'/' => encoded.push_str("~1"),
            other => encoded.push(other as char),
        }
    }
    encoded
}

/// `cJSONUtils_SortObject`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_SortObject(object: *mut cJSON) {
    unsafe { sort_object_impl(object, 0) }
}

/// `cJSONUtils_SortObjectCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_SortObjectCaseSensitive(object: *mut cJSON) {
    unsafe { sort_object_impl(object, 1) }
}

unsafe fn sort_object_impl(object: *mut cJSON, case_sensitive: cJSON_bool) {
    if object.is_null() || ((*object).ctype & 0xFF) != CJSON_OBJECT {
        return;
    }
    let mut nodes = Vec::new();
    let mut child = (*object).child;
    while !child.is_null() {
        nodes.push(child);
        child = (*child).next;
    }
    nodes.sort_by(|&a, &b| unsafe {
        let a_name = (*a).string;
        let b_name = (*b).string;
        if a_name.is_null() || b_name.is_null() {
            core::cmp::Ordering::Equal
        } else if case_sensitive != 0 {
            c_strcmp(a_name as *const u8, b_name as *const u8).cmp(&0)
        } else {
            case_insensitive_strcmp(a_name as *const u8, b_name as *const u8).cmp(&0)
        }
    });
    // relink the sorted list, preserving the head->prev == tail invariant
    for i in 0..nodes.len() {
        (*nodes[i]).next = if i + 1 < nodes.len() { nodes[i + 1] } else { ptr::null_mut() };
        (*nodes[i]).prev = if i > 0 { nodes[i - 1] } else { *nodes.last().unwrap() };
    }
    if !nodes.is_empty() {
        (*object).child = nodes[0];
    }
}

/// `cJSONUtils_AddPatchToArray`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_AddPatchToArray(
    array: *mut cJSON,
    operation: *const c_char,
    path: *const c_char,
    value: *const cJSON,
) -> cJSON_bool {
    unsafe {
        if array.is_null() {
            return 0;
        }
        let patch = new_item();
        if patch.is_null() {
            return 0;
        }
        (*patch).ctype = CJSON_OBJECT;
        let op_node = new_item();
        if !op_node.is_null() {
            (*op_node).ctype = CJSON_STRING;
            if !operation.is_null() {
                (*op_node).valuestring = c_json_strdup_impl(operation as *const u8, ptr::null()) as *mut c_char;
            }
            add_item_to_array_impl(patch, op_node);
            (*op_node).string = c_string("op") as *mut c_char;
        }
        let path_node = new_item();
        if !path_node.is_null() {
            (*path_node).ctype = CJSON_STRING;
            if !path.is_null() {
                (*path_node).valuestring = c_json_strdup_impl(path as *const u8, ptr::null()) as *mut c_char;
            }
            add_item_to_array_impl(patch, path_node);
            (*path_node).string = c_string("path") as *mut c_char;
        }
        if !value.is_null() {
            let value_node = duplicate_rec(value, 0, true);
            if !value_node.is_null() {
                add_item_to_array_impl(patch, value_node);
                (*value_node).string = c_string("value") as *mut c_char;
            }
        }
        add_item_to_array_impl(array, patch)
    }
}

// ---------------------------------------------------------------------------
// cJSON_Utils delegated through the safe Value model
// ---------------------------------------------------------------------------

/// `cJSONUtils_GeneratePatches`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_GeneratePatches(from: *const cJSON, to: *const cJSON) -> *mut cJSON {
    unsafe { generate_patches_impl(from, to, 0) }
}

/// `cJSONUtils_GeneratePatchesCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_GeneratePatchesCaseSensitive(from: *const cJSON, to: *const cJSON) -> *mut cJSON {
    unsafe { generate_patches_impl(from, to, 1) }
}

unsafe fn generate_patches_impl(from: *const cJSON, to: *const cJSON, case_sensitive: cJSON_bool) -> *mut cJSON {
    if from.is_null() || to.is_null() {
        return ptr::null_mut();
    }
    let from_value = node_to_value(from);
    let to_value = node_to_value(to);
    let patches = crate::generate_patches(&from_value, &to_value, case_sensitive != 0);
    value_to_node(&patches)
}

/// `cJSONUtils_ApplyPatches`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_ApplyPatches(object: *mut cJSON, patches: *const cJSON) -> c_int {
    unsafe { apply_patches_impl(object, patches, 0) }
}

/// `cJSONUtils_ApplyPatchesCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_ApplyPatchesCaseSensitive(object: *mut cJSON, patches: *const cJSON) -> c_int {
    unsafe { apply_patches_impl(object, patches, 1) }
}

unsafe fn apply_patches_impl(object: *mut cJSON, patches: *const cJSON, case_sensitive: cJSON_bool) -> c_int {
    if object.is_null() || patches.is_null() {
        return 1;
    }
    let mut object_value = node_to_value(object);
    let patches_value = node_to_value(patches);
    match crate::apply_patches(&mut object_value, &patches_value, case_sensitive != 0) {
        Ok(()) => {
            overwrite_node(object, &object_value);
            0
        }
        Err(status) => status.0 as i32,
    }
}

/// `cJSONUtils_MergePatch`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_MergePatch(target: *mut cJSON, patch: *const cJSON) -> *mut cJSON {
    unsafe { merge_patch_impl(target, patch, 0) }
}

/// `cJSONUtils_MergePatchCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_MergePatchCaseSensitive(target: *mut cJSON, patch: *const cJSON) -> *mut cJSON {
    unsafe { merge_patch_impl(target, patch, 1) }
}

unsafe fn merge_patch_impl(target: *mut cJSON, patch: *const cJSON, case_sensitive: cJSON_bool) -> *mut cJSON {
    if target.is_null() {
        if patch.is_null() {
            return ptr::null_mut();
        }
        return duplicate_rec(patch, 0, true);
    }
    if patch.is_null() {
        // matching cJSON.c: a null patch deletes the target
        delete_node(target);
        return ptr::null_mut();
    }
    let target_value = node_to_value(target);
    let patch_value = node_to_value(patch);
    let merged = crate::merge_patch(&target_value, &patch_value, case_sensitive != 0);
    overwrite_node(target, &merged);
    target
}

/// `cJSONUtils_GenerateMergePatch`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_GenerateMergePatch(from: *const cJSON, to: *const cJSON) -> *mut cJSON {
    unsafe { generate_merge_patch_impl(from, to, 0) }
}

/// `cJSONUtils_GenerateMergePatchCaseSensitive`
#[unsafe(no_mangle)]
pub extern "C" fn cJSONUtils_GenerateMergePatchCaseSensitive(from: *const cJSON, to: *const cJSON) -> *mut cJSON {
    unsafe { generate_merge_patch_impl(from, to, 1) }
}

unsafe fn generate_merge_patch_impl(from: *const cJSON, to: *const cJSON, case_sensitive: cJSON_bool) -> *mut cJSON {
    if from.is_null() || to.is_null() {
        return ptr::null_mut();
    }
    let from_value = node_to_value(from);
    let to_value = node_to_value(to);
    let patch = crate::generate_merge_patch(&from_value, &to_value, case_sensitive != 0);
    value_to_node(&patch)
}
