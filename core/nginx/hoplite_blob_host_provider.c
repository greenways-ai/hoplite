#include "hoplite_blob_host_provider.h"

#include <stdint.h>
#include <string.h>

#include "hoplite_blob_store_provider.h"
#include "hoplite_host_provider.h"

enum {
    HOPLITE_BLOB_HOST_NEW = 0,
    HOPLITE_BLOB_HOST_READY = 1,
    HOPLITE_BLOB_HOST_FAILED = 2,
    HOPLITE_BLOB_HOST_CLOSED = 3
};

static hoplite_blob_store_provider_t *hoplite_blob_provider;
static int hoplite_blob_state;

static int32_t
hoplite_blob_host_invoke(const hoplite_host_call_v1_t *call)
{
    hoplite_blob_store_call_v1_t blob_call;
    hoplite_blob_store_result_v1_t result;
    hoplite_host_complete_v1_pt complete;
    int32_t status;
    int32_t completion_status;

    if (call == NULL
        || call->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION
        || hoplite_blob_state != HOPLITE_BLOB_HOST_READY
        || hoplite_blob_provider == NULL
        || call->request_context == NULL
        || call->work == 0
        || call->operation.data == NULL
        || call->operation.len == 0
        || call->arguments_hta.data == NULL
        || call->arguments_hta.len == 0
        || call->completer.succeed == NULL
        || call->completer.fail == NULL)
    {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    blob_call.abi_version = HOPLITE_BLOB_STORE_PROVIDER_ABI_VERSION;
    blob_call.request_context = call->request_context;
    blob_call.work = call->work;
    blob_call.request_read = hoplite_host_request_body_read_v1;
    blob_call.request_finish = hoplite_host_request_body_finish_v1;

    memset(&result, 0, sizeof(result));
    status = hoplite_blob_store_provider_execute_v1(
        hoplite_blob_provider,
        &blob_call,
        call->operation.data,
        call->operation.len,
        call->arguments_hta.data,
        call->arguments_hta.len,
        &result);
    if (status != HOPLITE_BLOB_STORE_PROVIDER_OK) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (result.data == NULL || result.len == 0
        || (result.kind != HOPLITE_BLOB_STORE_RESULT_SUCCESS
            && result.kind != HOPLITE_BLOB_STORE_RESULT_FAILURE))
    {
        hoplite_blob_store_provider_result_free_v1(&result);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    complete = result.kind == HOPLITE_BLOB_STORE_RESULT_SUCCESS
        ? call->completer.succeed
        : call->completer.fail;
    completion_status = complete(
        call->completer.context,
        result.data,
        result.len);
    hoplite_blob_store_provider_result_free_v1(&result);

    return completion_status == HOPLITE_HOST_PROVIDER_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static const uint8_t hoplite_blob_service_name[] = "hara.blob";

static const hoplite_host_provider_v1_t hoplite_blob_host_provider = {
    HOPLITE_HOST_PROVIDER_ABI_VERSION,
    {
        hoplite_blob_service_name,
        sizeof(hoplite_blob_service_name) - 1
    },
    hoplite_blob_host_invoke,
    NULL,
    HOPLITE_HOST_PROVIDER_REQUEST_BODY | HOPLITE_HOST_PROVIDER_RESPONSE_BODY
};

int32_t
hoplite_blob_host_provider_init_process_v1(void)
{
    hoplite_blob_store_limits_v1_t limits;
    hoplite_blob_store_provider_t *provider = NULL;
    int32_t status;

    if (hoplite_blob_state == HOPLITE_BLOB_HOST_READY) {
        return HOPLITE_BLOB_HOST_PROVIDER_OK;
    }
    if (hoplite_blob_state != HOPLITE_BLOB_HOST_NEW
        || hoplite_blob_store_provider_abi_version()
            != HOPLITE_BLOB_STORE_PROVIDER_ABI_VERSION)
    {
        hoplite_blob_state = HOPLITE_BLOB_HOST_FAILED;
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }

    limits.max_object_bytes = 16u * 1024u * 1024u;
    limits.max_append_bytes = 1024u * 1024u;
    limits.max_source_chunk_bytes = 64u * 1024u;
    limits.max_staging_key_bytes = 256u;
    limits.max_media_type_bytes = 256u;
    limits.max_staging_entries = 1024u;
    limits.max_objects = 65536u;

    status = hoplite_blob_store_provider_open_memory_v1(&limits, &provider);
    if (status != HOPLITE_BLOB_STORE_PROVIDER_OK || provider == NULL) {
        hoplite_blob_state = HOPLITE_BLOB_HOST_FAILED;
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }
    status = hoplite_host_provider_register_v1(&hoplite_blob_host_provider);
    if (status != HOPLITE_HOST_PROVIDER_REGISTER_OK) {
        hoplite_blob_store_provider_close_v1(provider);
        hoplite_blob_state = HOPLITE_BLOB_HOST_FAILED;
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }

    hoplite_blob_provider = provider;
    hoplite_blob_state = HOPLITE_BLOB_HOST_READY;
    return HOPLITE_BLOB_HOST_PROVIDER_OK;
}

size_t
hoplite_blob_host_provider_release_work_v1(uint64_t work)
{
    if (hoplite_blob_state != HOPLITE_BLOB_HOST_READY
        || hoplite_blob_provider == NULL || work == 0)
    {
        return 0;
    }
    return hoplite_blob_store_provider_release_work_v1(
        hoplite_blob_provider,
        work);
}

void
hoplite_blob_host_provider_exit_process_v1(void)
{
    if (hoplite_blob_provider != NULL) {
        hoplite_blob_store_provider_close_v1(hoplite_blob_provider);
        hoplite_blob_provider = NULL;
    }
    if (hoplite_blob_state == HOPLITE_BLOB_HOST_READY) {
        hoplite_blob_state = HOPLITE_BLOB_HOST_CLOSED;
    }
}
