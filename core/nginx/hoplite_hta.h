#ifndef HOPLITE_HTA_H
#define HOPLITE_HTA_H

#include <ngx_config.h>
#include <ngx_core.h>
#include <ngx_http.h>

/* Exact Hara wire prefix shared by the Nginx decoder and native providers. */
#define HOPLITE_HTA_MAGIC_BYTES 'H', 'T', 'A', '0'

/*
 * Keep decoded kinds numerically aligned with their canonical HTA0 wire tags.
 * Native providers also write exact HTA0 frames and may use these tag values
 * while classifying decoded arguments. Explicit values prevent a provider-local
 * wire-tag declaration from silently disagreeing with the shared decoder.
 * Boolean values share one decoded kind even though false and true use tags 1
 * and 2 respectively.
 */
typedef enum {
    HOPLITE_HTA_NIL = 0u,
    HOPLITE_HTA_BOOL = 1u,
    HOPLITE_HTA_I64 = 3u,
    HOPLITE_HTA_STRING = 4u,
    HOPLITE_HTA_BYTES = 5u,
    HOPLITE_HTA_KEYWORD = 6u,
    HOPLITE_HTA_VECTOR = 9u,
    HOPLITE_HTA_MAP = 11u,
    HOPLITE_HTA_ARRAY = 17u,
    HOPLITE_HTA_OBJECT = 18u
} hoplite_hta_kind_t;

typedef struct hoplite_hta_value hoplite_hta_value_t;

typedef struct {
    hoplite_hta_value_t *key;
    hoplite_hta_value_t *value;
} hoplite_hta_pair_t;

struct hoplite_hta_value {
    hoplite_hta_kind_t kind;
    const u_char *encoded;
    size_t encoded_len;
    union {
        ngx_flag_t boolean;
        int64_t i64;
        ngx_str_t text;
        struct {
            size_t count;
            hoplite_hta_value_t **items;
        } vector;
        struct {
            size_t count;
            hoplite_hta_pair_t *items;
        } map;
    } as;
};

ngx_int_t hoplite_hta_decode(ngx_pool_t *pool,
                             const u_char *data,
                             size_t len,
                             hoplite_hta_value_t **value);

/* Copy one decoded value into its exact standalone HTA0 frame. */
ngx_int_t hoplite_hta_copy_frame(ngx_pool_t *pool,
                                 const hoplite_hta_value_t *value,
                                 ngx_str_t *output);

ngx_int_t hoplite_hta_encode_request(ngx_http_request_t *request,
                                     ngx_str_t *output);

ngx_int_t hoplite_hta_encode_string(ngx_pool_t *pool,
                                    const ngx_str_t *value,
                                    ngx_str_t *output);

ngx_int_t hoplite_hta_encode_number(ngx_pool_t *pool,
                                    int64_t value,
                                    ngx_str_t *output);

hoplite_hta_value_t *hoplite_hta_map_get(const hoplite_hta_value_t *map,
                                         const char *name);

ngx_int_t hoplite_hta_text(const hoplite_hta_value_t *value,
                           ngx_str_t *output);

ngx_int_t hoplite_hta_number(const hoplite_hta_value_t *value,
                             int64_t *output);

#endif
