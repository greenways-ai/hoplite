#include "hoplite_host_provider.h"

#include <assert.h>
#include <stdint.h>
#include <string.h>

static int32_t
complete(void *context, const uint8_t *hta, size_t hta_len)
{
    size_t *completed = context;
    assert(hta_len == 0 || hta != NULL);
    (*completed)++;
    return HOPLITE_HOST_PROVIDER_OK;
}

static int32_t
invoke(const hoplite_host_call_v1_t *call)
{
    assert(call != NULL);
    assert(call->abi_version == HOPLITE_HOST_PROVIDER_ABI_VERSION);
    assert(call->work == 7);
    assert(call->call == 11);
    assert(call->operation.len == 4);
    assert(memcmp(call->operation.data, "load", 4) == 0);
    assert(call->arguments_hta.len == 5);
    assert(memcmp(call->arguments_hta.data, "HTA1\0", 5) == 0);
    assert(call->arguments_value == NULL);
    return call->completer.succeed(
        call->completer.context,
        call->arguments_hta.data,
        call->arguments_hta.len);
}

static void
cancel(void *request_context)
{
    size_t *cancelled = request_context;
    (*cancelled)++;
}

static hoplite_host_service_t
service(const char *value)
{
    hoplite_host_service_t result;
    result.data = (const uint8_t *) value;
    result.len = strlen(value);
    return result;
}

int
main(void)
{
    size_t completed = 0;
    size_t cancelled = 0;
    hoplite_host_provider_v1_t provider = {
        HOPLITE_HOST_PROVIDER_ABI_VERSION,
        {(const uint8_t *) "tahto.metadata", sizeof("tahto.metadata") - 1},
        invoke,
        cancel,
        0
    };
    hoplite_host_provider_v1_t incompatible = provider;
    hoplite_host_call_v1_t call = {
        HOPLITE_HOST_PROVIDER_ABI_VERSION,
        &cancelled,
        7,
        11,
        {(const uint8_t *) "load", sizeof("load") - 1},
        {(const uint8_t *) "HTA1\0", 5},
        NULL,
        {&completed, complete, complete}
    };

    assert(hoplite_host_provider_find_v1(provider.service) == NULL);
    assert(hoplite_host_provider_register_v1(&provider)
           == HOPLITE_HOST_PROVIDER_REGISTER_OK);
    assert(hoplite_host_provider_find_v1(provider.service) == &provider);
    assert(hoplite_host_provider_find_v1(service("TAHTO.METADATA")) == NULL);
    assert(hoplite_host_provider_register_v1(&provider)
           == HOPLITE_HOST_PROVIDER_REGISTER_DUPLICATE);

    incompatible.abi_version++;
    incompatible.service = service("future.provider");
    assert(hoplite_host_provider_register_v1(&incompatible)
           == HOPLITE_HOST_PROVIDER_REGISTER_ABI_MISMATCH);
    assert(hoplite_host_provider_register_v1(NULL)
           == HOPLITE_HOST_PROVIDER_REGISTER_INVALID);

    assert(provider.invoke(&call) == HOPLITE_HOST_PROVIDER_OK);
    assert(completed == 1);
    provider.cancel(call.request_context);
    assert(cancelled == 1);
    assert(HOPLITE_HOST_PROVIDER_REQUEST_BODY
           != HOPLITE_HOST_PROVIDER_RESPONSE_BODY);
    return 0;
}
