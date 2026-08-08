#ifndef HOPLITE_BLOB_HOST_PROVIDER_H
#define HOPLITE_BLOB_HOST_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#define HOPLITE_BLOB_HOST_PROVIDER_OK 0
#define HOPLITE_BLOB_HOST_PROVIDER_ERROR (-1)

/* Construct and register one in-memory hara.blob provider for this worker. */
int32_t hoplite_blob_host_provider_init_process_v1(void);

/* Release immutable response sources retained by one completed work. */
size_t hoplite_blob_host_provider_release_work_v1(uint64_t work);

/* Close every retained source and the worker-owned provider exactly once. */
void hoplite_blob_host_provider_exit_process_v1(void);

#endif /* HOPLITE_BLOB_HOST_PROVIDER_H */
