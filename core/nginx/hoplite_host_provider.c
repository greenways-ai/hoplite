#include "hoplite_host_provider.h"

static hoplite_host_registry_t hoplite_provider_registry;
static int hoplite_provider_registry_ready;

static void
hoplite_host_provider_ensure_registry(void)
{
    if (!hoplite_provider_registry_ready) {
        hoplite_host_registry_init(&hoplite_provider_registry);
        hoplite_provider_registry_ready = 1;
    }
}

int32_t
hoplite_host_provider_register_v1(
    const hoplite_host_provider_v1_t *provider)
{
    int result;

    if (provider == NULL
        || provider->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION)
    {
        return provider == NULL
            ? HOPLITE_HOST_PROVIDER_REGISTER_INVALID
            : HOPLITE_HOST_PROVIDER_REGISTER_ABI_MISMATCH;
    }
    if (provider->invoke == NULL) {
        return HOPLITE_HOST_PROVIDER_REGISTER_INVALID;
    }
    if ((provider->capabilities & HOPLITE_HOST_PROVIDER_RESPONSE_BODY) != 0
        && (provider->response_read == NULL
            || provider->response_close == NULL
            || provider->release_work == NULL))
    {
        return HOPLITE_HOST_PROVIDER_REGISTER_INVALID;
    }

    hoplite_host_provider_ensure_registry();
    result = hoplite_host_registry_register(
        &hoplite_provider_registry,
        provider->service,
        provider);

    switch (result) {
    case HOPLITE_HOST_REGISTRY_OK:
        return HOPLITE_HOST_PROVIDER_REGISTER_OK;
    case HOPLITE_HOST_REGISTRY_DUPLICATE:
        return HOPLITE_HOST_PROVIDER_REGISTER_DUPLICATE;
    case HOPLITE_HOST_REGISTRY_FULL:
        return HOPLITE_HOST_PROVIDER_REGISTER_FULL;
    default:
        return HOPLITE_HOST_PROVIDER_REGISTER_INVALID;
    }
}

const hoplite_host_provider_v1_t *
hoplite_host_provider_find_v1(hoplite_host_service_t service)
{
    if (!hoplite_provider_registry_ready) {
        return NULL;
    }
    return hoplite_host_registry_find(&hoplite_provider_registry, service);
}
