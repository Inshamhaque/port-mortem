/*
 * Differential-fuzz oracle over the FFI layer.
 *
 * Reads one JSON document from stdin, parses it with cJSON_ParseWithOpts
 * (require_null_terminated = 0, exactly what cJSON_Parse does) and prints a
 * canonical, line-oriented verdict that fuzz/harness.py diffs against the safe
 * Rust core:
 *
 *   OK <consumed> <compact JSON>   -- parse succeeded, <consumed> bytes used
 *   ERR <offset>                   -- parse failed, error offset (-1 if unknown)
 *   DRIVER_ERR <reason>            -- oracle itself broke (skip this input)
 *
 * The <consumed> byte count lets the harness treat cJSON's "ignore trailing
 * content" semantics explicitly instead of letting them look like divergences
 * against the safe parser, which deliberately requires whole-input consumption.
 *
 * Built from libcjson_rs.a (the Rust FFI layer), so this oracle exercises the
 * same C symbols the original test suite calls.
 */

#include "cJSON.h"

#include <stdio.h>
#include <stdlib.h>

int main(void)
{
    const size_t cap = 1u << 20; /* reject inputs larger than 1 MiB in the driver */
    char *buf = (char *)malloc(cap);
    size_t len = 0;
    int ch;

    if (buf == NULL)
    {
        fprintf(stderr, "driver: allocation failed\n");
        return 3;
    }

    while ((ch = getchar()) != EOF && len + 1 < cap)
    {
        buf[len++] = (char)ch;
    }
    buf[len] = '\0';

    const char *parse_end = NULL;
    cJSON *tree = cJSON_ParseWithOpts(buf, &parse_end, 0);

    if (tree == NULL)
    {
        const char *err = cJSON_GetErrorPtr();
        long offset = (err != NULL) ? (long)(err - buf) : -1;
        printf("ERR %ld\n", offset);
        free(buf);
        return 0;
    }

    long consumed = (parse_end != NULL) ? (long)(parse_end - buf) : (long)len;
    char *printed = cJSON_PrintUnformatted(tree);
    if (printed == NULL)
    {
        printf("DRIVER_ERR print\n");
    }
    else
    {
        printf("OK %ld %s\n", consumed, printed);
        free(printed);
    }
    cJSON_Delete(tree);
    free(buf);
    return 0;
}
