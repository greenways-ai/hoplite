#ifndef HOPLITE_BLOB_HOST_PROVIDER_H
#define HOPLITE_BLOB_HOST_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#define HOPLITE_BLOB_HOST_PROVIDER_OK 0
#define HOPLITE_BLOB_HOST_PROVIDER_ERROR (-1)

/*
 * Construct and register one hoplite.blob provider for this worker.
 *
 * Trusted startup configuration selects the restart-safe filesystem driver
 * when HOPLITE_BLOB_ROOT is non-empty. An absent root retains the
 * in-memory driver for one compatibility cycle and deterministic development.
 * HAL calls cannot select the driver, root or limits.
 */
int32_t hoplite_blob_host_provider_init_process_v1(void);

/*
 * Read or close an immutable source only through its exact owning request and
 * work. A numeric source handle alone grants no access.
 */
int32_t hoplite_blob_host_provider_response_read_v1(
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

int32_t hoplite_blob_host_provider_response_close_v1(
    void *request_context,
    uint64_t work,
    uint64_t source_handle);

/* Release immutable response sources retained by one completed work. */
size_t hoplite_blob_host_provider_release_work_v1(uint64_t work);

/* Close every retained source and the worker-owned provider exactly once. */
void hoplite_blob_host_provider_exit_process_v1(void);

#endif /* HOPLITE_BLOB_HOST_PROVIDER_H */
