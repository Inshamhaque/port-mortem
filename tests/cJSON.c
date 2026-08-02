/*
 * port-mortem test shim (implementation half).
 *
 * The original cJSON tests #include "../cJSON.c" (see common.h and the parse/
 * print test files). In the upstream repo that pulls in the real library
 * source; here it pulls in THIS file, which only *declares* the C surface that
 * src/ffi.rs exports with C linkage. All definitions live in libcjson_rs.a
 * (built from the Rust crate) and are linked in by the test build.
 *
 * What this file provides:
 *   - the cJSON.c-internal structs (internal_hooks, parse_buffer, printbuffer)
 *     with field order/types matching the #[repr(C)] structs in src/ffi.rs,
 *   - the extern for `global_hooks` (defined + exported by the Rust side),
 *   - extern declarations for the white-box internals the tests call directly.
 *
 * The public API is declared in cJSON.h (included below). Nothing in here is
 * a definition, so no symbol collides with the Rust library.
 */

#include "cJSON.h"

#include <stdbool.h>
#include <string.h>
#include <stdio.h>
#include <math.h>
#include <stdlib.h>
#include <float.h>
#include <limits.h>
#include <ctype.h>

/* cJSON.c-internal allocator hook struct, matching src/ffi.rs `internal_hooks`. */
typedef struct internal_hooks
{
    void *(CJSON_CDECL *allocate)(size_t size);
    void (CJSON_CDECL *deallocate)(void *pointer);
    void *(CJSON_CDECL *reallocate)(void *pointer, size_t size);
} internal_hooks;

/* The active allocator. Defined in Rust (ffi.rs `global_hooks`) and exported. */
extern internal_hooks global_hooks;

/* cJSON.c-internal parse state, matching src/ffi.rs `parse_buffer`. */
typedef struct parse_buffer
{
    const unsigned char *content;
    size_t length;
    size_t offset;
    size_t depth;
    internal_hooks hooks;
} parse_buffer;

/* cJSON.c-internal print state, matching src/ffi.rs `printbuffer`. */
typedef struct printbuffer
{
    unsigned char *buffer;
    size_t length;
    size_t offset;
    size_t depth;
    cJSON_bool noalloc;
    cJSON_bool format;
    internal_hooks hooks;
} printbuffer;

/* White-box internals, exported from src/ffi.rs for the original tests. */
extern unsigned int parse_hex4(const unsigned char *input);
extern cJSON_bool parse_number(cJSON *item, parse_buffer *input_buffer);
extern cJSON_bool parse_string(cJSON *item, parse_buffer *input_buffer);
extern cJSON_bool parse_array(cJSON *item, parse_buffer *input_buffer);
extern cJSON_bool parse_object(cJSON *item, parse_buffer *input_buffer);
extern cJSON_bool parse_value(cJSON *item, parse_buffer *input_buffer);
extern cJSON_bool print_value(const cJSON *item, printbuffer *output_buffer);
extern cJSON_bool print_array(const cJSON *item, printbuffer *output_buffer);
extern cJSON_bool print_object(const cJSON *item, printbuffer *output_buffer);
extern cJSON_bool print_number(const cJSON *item, printbuffer *output_buffer);
extern cJSON_bool print_string_ptr(const unsigned char *input, printbuffer *output_buffer);
extern unsigned char *ensure(printbuffer *output_buffer, size_t needed);
extern parse_buffer *skip_utf8_bom(parse_buffer *buffer);
extern cJSON_bool add_item_to_array(cJSON *array, cJSON *item);
extern cJSON_bool compare_double(double a, double b);

/* cJSON.c-internal string helper, called directly by misc_tests.c. */
extern unsigned char *cJSON_strdup(const unsigned char *string, const internal_hooks *hooks);
