#ifndef HOPLITE_VALUE_STORE_HOST_PROVIDER_H
#define HOPLITE_VALUE_STORE_HOST_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#define HOPLITE_VALUE_STORE_HOST_PROVIDER_OK 0
#define HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED 1
#define HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR (-1)

/*
 * Register one worker-owned SQLite hara.store provider from trusted bytes.
 * This function is a startup boundary; request and application values must
 * never supply the path, driver choice, or limits.
 */
int32_t hoplite_value_store_host_provider_register_sqlite_v1(
    const uint8_t *path,
    size_t path_len,
    size_t max_value_bytes,
    size_t max_receipt_bytes);

/*
 * Read trusted process configuration and register the provider once.
 *
 * HOPLITE_HARA_STORE_PATH enables the provider. Optional positive decimal
 * limits are read from HOPLITE_HARA_STORE_MAX_VALUE_BYTES and
 * HOPLITE_HARA_STORE_MAX_RECEIPT_BYTES. An absent path leaves the service
 * intentionally disabled; malformed or unusable configuration is an error.
 */
int32_t hoplite_value_store_host_provider_init_process_v1(void);

/* Close the worker-owned provider exactly once. Safe to call repeatedly. */
void hoplite_value_store_host_provider_exit_process_v1(void);

#endif /* HOPLITE_VALUE_STORE_HOST_PROVIDER_H */
