#ifndef HOPLITE_VALUE_STORE_HOST_PROVIDER_H
#define HOPLITE_VALUE_STORE_HOST_PROVIDER_H

#include "hoplite_store_host_provider.h"

#define HOPLITE_VALUE_STORE_HOST_PROVIDER_OK \
    HOPLITE_STORE_HOST_PROVIDER_OK
#define HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED \
    HOPLITE_STORE_HOST_PROVIDER_DISABLED
#define HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR \
    HOPLITE_STORE_HOST_PROVIDER_ERROR

/*
 * Compatibility entry point for trusted callers that previously registered
 * hoplite.store through the combined store+value bootstrap.
 */
int32_t hoplite_value_store_host_provider_register_sqlite_v1(
    const uint8_t *path,
    size_t path_len,
    size_t max_value_bytes,
    size_t max_receipt_bytes);

/*
 * Initialise the store provider and then the independently configured
 * hoplite.value provider. New store-only distributions should call
 * hoplite_store_host_provider_init_process_v1 directly.
 */
int32_t hoplite_value_store_host_provider_init_process_v1(void);

/* Close hoplite.value and then hoplite.store. Safe to call repeatedly. */
void hoplite_value_store_host_provider_exit_process_v1(void);

#endif /* HOPLITE_VALUE_STORE_HOST_PROVIDER_H */
