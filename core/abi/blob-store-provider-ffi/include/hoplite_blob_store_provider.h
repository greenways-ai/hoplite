#ifndef HOPLITE_BLOB_STORE_PROVIDER_H
#define HOPLITE_BLOB_STORE_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HOPLITE_BLOB_STORE_PROVIDER_ABI_VERSION 1u

#define HOPLITE_BLOB_STORE_PROVIDER_OK 0
#define HOPLITE_BLOB_STORE_PROVIDER_INVALID 1
#define HOPLITE_BLOB_STORE_PROVIDER_FAILURE 2
#define HOPLITE_BLOB_STORE_PROVIDER_RESOURCE_ERROR 3

#define HOPLITE_BLOB_STORE_RESULT_SUCCESS 1u
#define HOPLITE_BLOB_STORE_RESULT_FAILURE 2u

typedef struct hoplite_blob_store_provider hoplite_blob_store_provider_t;

typedef int32_t (*hoplite_blob_request_read_v1)(
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

typedef int32_t (*hoplite_blob_request_finish_v1)(
    void *request_context,
    uint64_t work,
    uint64_t source_handle);

typedef struct hoplite_blob_store_limits_v1 {
    uint64_t max_object_bytes;
    size_t max_append_bytes;
    size_t max_source_chunk_bytes;
    size_t max_staging_key_bytes;
    size_t max_media_type_bytes;
    size_t max_staging_entries;
    size_t max_objects;
} hoplite_blob_store_limits_v1_t;

typedef struct hoplite_blob_store_call_v1 {
    uint32_t abi_version;
    void *request_context;
    uint64_t work;
    hoplite_blob_request_read_v1 request_read;
    hoplite_blob_request_finish_v1 request_finish;
} hoplite_blob_store_call_v1_t;

typedef struct hoplite_blob_store_result_v1 {
    uint32_t kind;
    uint8_t *data;
    size_t len;
} hoplite_blob_store_result_v1_t;

uint32_t hoplite_blob_store_provider_abi_version(void);

/*
 * Create one worker-owned, application-neutral in-memory provider for
 * deterministic tests and development. The caller supplies trusted positive
 * limits; Hara values cannot select the driver or mutate these limits.
 */
int32_t hoplite_blob_store_provider_open_memory_v1(
    const hoplite_blob_store_limits_v1_t *limits,
    hoplite_blob_store_provider_t **provider);

/*
 * Create one worker-owned, application-neutral trusted-root filesystem
 * provider. The UTF-8 root and fixed limits come only from trusted startup
 * configuration. A HAL request cannot select or modify either value.
 */
int32_t hoplite_blob_store_provider_open_filesystem_v1(
    const uint8_t *root,
    size_t root_len,
    const hoplite_blob_store_limits_v1_t *limits,
    hoplite_blob_store_provider_t **provider);

/*
 * Execute one exact hara.blob operation over a standalone canonical HTA1
 * argument frame. Request source callbacks are already bound by the host to
 * the exact request and work. The provider returns an owned canonical result
 * frame or a closed stable error-code string.
 */
int32_t hoplite_blob_store_provider_execute_v1(
    hoplite_blob_store_provider_t *provider,
    const hoplite_blob_store_call_v1_t *call,
    const uint8_t *operation,
    size_t operation_len,
    const uint8_t *arguments_hta,
    size_t arguments_hta_len,
    hoplite_blob_store_result_v1_t *result);

/*
 * Read or close an immutable response source registered by object/open-source.
 * The exact owning work must match; a numeric handle alone is never authority.
 */
int32_t hoplite_blob_store_provider_response_read_v1(
    hoplite_blob_store_provider_t *provider,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

int32_t hoplite_blob_store_provider_response_close_v1(
    hoplite_blob_store_provider_t *provider,
    uint64_t work,
    uint64_t source_handle);

/* Close every response source retained by one completed or cancelled work. */
size_t hoplite_blob_store_provider_release_work_v1(
    hoplite_blob_store_provider_t *provider,
    uint64_t work);

void hoplite_blob_store_provider_result_free_v1(
    hoplite_blob_store_result_v1_t *result);

void hoplite_blob_store_provider_close_v1(
    hoplite_blob_store_provider_t *provider);

#ifdef __cplusplus
}
#endif

#endif /* HOPLITE_BLOB_STORE_PROVIDER_H */
