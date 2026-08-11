#ifndef HOPLITE_VALUE_STORE_PROVIDER_H
#define HOPLITE_VALUE_STORE_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HOPLITE_VALUE_STORE_PROVIDER_ABI_VERSION 0u

#define HOPLITE_VALUE_STORE_PROVIDER_OK 0
#define HOPLITE_VALUE_STORE_PROVIDER_INVALID 1
#define HOPLITE_VALUE_STORE_PROVIDER_OPEN_ERROR 2
#define HOPLITE_VALUE_STORE_PROVIDER_PANIC 3

#define HOPLITE_VALUE_STORE_RESULT_SUCCESS 0u
#define HOPLITE_VALUE_STORE_RESULT_FAILURE 1u

typedef struct hoplite_value_store_provider hoplite_value_store_provider_t;

typedef struct {
    uint8_t *data;
    size_t len;
    uint32_t kind;
} hoplite_value_store_result_v1_t;

uint32_t hoplite_value_store_provider_abi_version(void);

/*
 * Open one worker-owned SQLite provider from trusted startup configuration.
 * The database path and limits must not originate in a Hara request.
 */
int32_t hoplite_value_store_provider_open_sqlite_v1(
    const uint8_t *path,
    size_t path_len,
    size_t max_value_bytes,
    size_t max_receipt_bytes,
    hoplite_value_store_provider_t **provider);

/*
 * Execute one exact hoplite.store operation and standalone HTA0 argument frame.
 *
 * A valid protocol request always returns HOPLITE_VALUE_STORE_PROVIDER_OK and
 * one owned result frame. kind distinguishes a successful hoplite.store result
 * from a closed failure frame containing only the stable generic error code.
 * ABI preflight and panic failures return a non-zero status and no frame.
 */
int32_t hoplite_value_store_provider_execute_v1(
    hoplite_value_store_provider_t *provider,
    const uint8_t *operation,
    size_t operation_len,
    const uint8_t *arguments_hta,
    size_t arguments_hta_len,
    hoplite_value_store_result_v1_t *result);

/* Release one result frame. Null and already-zero results are accepted. */
void hoplite_value_store_provider_result_free_v1(
    hoplite_value_store_result_v1_t *result);

/* Release one provider opened by open_sqlite_v1. Null is accepted. */
void hoplite_value_store_provider_close_v1(
    hoplite_value_store_provider_t *provider);

#ifdef __cplusplus
}
#endif

#endif
