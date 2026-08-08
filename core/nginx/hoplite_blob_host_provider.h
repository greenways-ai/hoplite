#ifndef HOPLITE_BLOB_HOST_PROVIDER_H
#define HOPLITE_BLOB_HOST_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#define HOPLITE_BLOB_HOST_PROVIDER_OK 0
#define HOPLITE_BLOB_HOST_PROVIDER_ERROR (-1)

/*
 * Construct and register one hara.blob provider for this worker.
 *
 * Trusted startup configuration selects the restart-safe filesystem driver
 * when HOPLITE_HARA_BLOB_ROOT is non-empty. An absent root retains the
 * in-memory driver for one compatibility cycle and deterministic development.
 * HAL calls cannot select the driver, root or limits.
 */
int32_t hoplite_blob_host_provider_init_process_v1(void);

/* Release immutable response sources retained by one completed work. */
size_t hoplite_blob_host_provider_release_work_v1(uint64_t work);

/* Close every retained source and the worker-owned provider exactly once. */
void hoplite_blob_host_provider_exit_process_v1(void);

#endif /* HOPLITE_BLOB_HOST_PROVIDER_H */
