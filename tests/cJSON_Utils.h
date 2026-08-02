/*
  Copyright (c) 2009-2017 Dave Gamble and cJSON contributors

  Permission is hereby granted, free of charge, to any person obtaining a copy
  of this software and associated documentation files (the "Software"), to deal
  in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  copies of the Software, and to permit persons to whom the Software is
  furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in
  all copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
  THE SOFTWARE.
*/

/*
 * Declaration half of the cJSON_Utils shim. Mirrors the original cJSON_Utils.h
 * from v1.7.19; every function is DECLARED extern because the definitions live
 * in the Rust FFI layer (src/ffi.rs) and are linked from libcjson_rs.a.
 */

#ifndef cJSON_Utils__h
#define cJSON_Utils__h

#include "cJSON.h"

#ifdef __cplusplus
extern "C"
{
#endif

/* Implement RFC6901 (https://tools.ietf.org/html/rfc6901) JSON Pointers. */
CJSON_PUBLIC(cJSON *) cJSONUtils_GetPointer(const cJSON * const object, const char *pointer);
CJSON_PUBLIC(cJSON *) cJSONUtils_GetPointerCaseSensitive(const cJSON * const object, const char *pointer);

/* Implement RFC6902 (https://tools.ietf.org/html/rfc6902) JSON Patch. */
CJSON_PUBLIC(cJSON *) cJSONUtils_GeneratePatches(cJSON * const from, cJSON * const to);
CJSON_PUBLIC(cJSON *) cJSONUtils_GeneratePatchesCaseSensitive(cJSON * const from, cJSON * const to);
/* Utility for generating patch array. */
CJSON_PUBLIC(cJSON_bool) cJSONUtils_AddPatchToArray(cJSON * const array, const char * const operation, const char * const path, const cJSON * const value);
/* Returns 0 for success. */
CJSON_PUBLIC(int) cJSONUtils_ApplyPatches(cJSON * const object, const cJSON * const patches);
CJSON_PUBLIC(int) cJSONUtils_ApplyPatchesCaseSensitive(cJSON * const object, const cJSON * const patches);

/*
 * Note that ApplyPatches is NOT atomic on failure. To undo an incomplete
 * application, use cJSONUtils_MergePatch with the original object and apply
 * the reverse of the patch.
 */

/* Implement RFC7386 (https://tools.ietf.org/html/rfc7396) JSON Merge Patch. */
CJSON_PUBLIC(cJSON *) cJSONUtils_MergePatch(cJSON *target, const cJSON * const patch);
CJSON_PUBLIC(cJSON *) cJSONUtils_MergePatchCaseSensitive(cJSON *target, const cJSON * const patch);
/* Memory management: the result will be freed with cJSON_Delete. */
CJSON_PUBLIC(cJSON *) cJSONUtils_GenerateMergePatch(cJSON * const from, cJSON * const to);
CJSON_PUBLIC(cJSON *) cJSONUtils_GenerateMergePatchCaseSensitive(cJSON * const from, cJSON * const to);

/* Given a source object and a target object, find the JSON Pointer path from source to target.
 * Returns NULL if the path cannot be found. */
CJSON_PUBLIC(char *) cJSONUtils_FindPointerFromObjectTo(const cJSON * const object, const cJSON * const target);

/* Sorts the members of the object. */
CJSON_PUBLIC(void) cJSONUtils_SortObject(cJSON * const object);
CJSON_PUBLIC(void) cJSONUtils_SortObjectCaseSensitive(cJSON * const object);

#ifdef __cplusplus
}
#endif

#endif
