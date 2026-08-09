#ifndef HOPLITE_VALUE_PROVIDER_H
#define HOPLITE_VALUE_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HOPLITE_VALUE_PROVIDER_ABI_VERSION 1u

#define HOPLITE_VALUE_PROVIDER_OK 0
#define HOPLITE_VALUE_PROVIDER_INVALID 1
#define HOPLITE_VALUE_PROVIDER_OPEN_ERROR 2
#define HOPLITE_VALUE_PROVIDER_PANIC 3

#define HOPLITE_VALUE_RESULT_SUCCESS 0u
#define HOPLITE_VALUE_RESULT_FAILURE 1u

typedef struct hoplite_value_provider hoplite_value_provider_t;

typedef struct {
    uint8_t *data;
    size_t len;
    uint32_t kind;
} hoplite_value_result_v1_t;

uint32_t hoplite_value_provider_abi_version(void);

/*
 * Open one worker-owned filesystem provider from trusted startup configuration.
 * The root and ceilings must never originate in a portable Hara request.
 */
int32_t hoplite_value_provider_open_filesystem_v1(
    const uint8_t *root,
    size_t root_len,
    size_t max_frame_bytes,
    size_t max_media_type_bytes,
    size_t io_chunk_bytes,
    hoplite_value_provider_t **provider);

/* Execute one exact hara.value operation and standalone HTA1 argument frame. */
int32_t hoplite_value_provider_execute_v1(
    hoplite_value_provider_t *provider,
    const uint8_t *operation,
    size_t operation_len,
    const uint8_t *arguments_hta,
    size_t arguments_hta_len,
    hoplite_value_result_v1_t *result);

void hoplite_value_provider_result_free_v1(
    hoplite_value_result_v1_t *result);

void hoplite_value_provider_close_v1(
    hoplite_value_provider_t *provider);

#ifdef __cplusplus
}
#endif

#endif
