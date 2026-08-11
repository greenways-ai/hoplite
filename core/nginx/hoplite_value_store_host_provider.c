#include "hoplite_value_store_host_provider.h"

#include "hoplite_value_host_provider.h"

int32_t
hoplite_value_store_host_provider_register_sqlite_v1(
    const uint8_t *path,
    size_t path_len,
    size_t max_value_bytes,
    size_t max_receipt_bytes)
{
    return hoplite_store_host_provider_register_sqlite_v1(
        path,
        path_len,
        max_value_bytes,
        max_receipt_bytes);
}

/*
 * This compatibility hook remains the installed-provider bootstrap
 * aggregator. The store and value providers retain separate configuration,
 * handles and registry entries, and store-only distributions do not link this
 * object or the hoplite.value provider.
 */
int32_t
hoplite_value_store_host_provider_init_process_v1(void)
{
    int32_t store_status;
    int32_t value_status;

    store_status = hoplite_store_host_provider_init_process_v1();
    if (store_status == HOPLITE_STORE_HOST_PROVIDER_ERROR) {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    value_status = hoplite_value_host_provider_init_process_v1();
    if (value_status == HOPLITE_VALUE_HOST_PROVIDER_ERROR) {
        hoplite_value_host_provider_exit_process_v1();
        hoplite_store_host_provider_exit_process_v1();
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    if (store_status == HOPLITE_STORE_HOST_PROVIDER_DISABLED
        && value_status == HOPLITE_VALUE_HOST_PROVIDER_DISABLED)
    {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED;
    }
    return HOPLITE_VALUE_STORE_HOST_PROVIDER_OK;
}

void
hoplite_value_store_host_provider_exit_process_v1(void)
{
    hoplite_value_host_provider_exit_process_v1();
    hoplite_store_host_provider_exit_process_v1();
}
