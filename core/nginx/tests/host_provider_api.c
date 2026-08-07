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
    hoplite_host_call_v1_t call = {
        HOPLITE_HOST_PROVIDER_ABI_VERSION,
        &cancelled,
        7,
        11,
        {(const uint8_t *) "load", sizeof("load") - 1},
        {(const uint8_t *) "HTA1\0", 5},
        {&completed, complete, complete}
    };

    assert(provider.abi_version == HOPLITE_HOST_PROVIDER_ABI_VERSION);
    assert(provider.service.len == sizeof("tahto.metadata") - 1);
    assert(provider.invoke(&call) == HOPLITE_HOST_PROVIDER_OK);
    assert(completed == 1);
    provider.cancel(call.request_context);
    assert(cancelled == 1);
    assert(HOPLITE_HOST_PROVIDER_REQUEST_BODY != HOPLITE_HOST_PROVIDER_RESPONSE_BODY);
    assert(HOPLITE_HOST_PROVIDER_REGISTER_ABI_MISMATCH
           != HOPLITE_HOST_PROVIDER_REGISTER_INVALID);
    return 0;
}
