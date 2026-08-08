#include "hoplite_blob_host_provider.h"

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "hoplite_blob_store_provider.h"
#include "hoplite_host_provider.h"

#define HOPLITE_HARA_BLOB_ROOT_ENV "HOPLITE_HARA_BLOB_ROOT"
#define HOPLITE_HARA_BLOB_MAX_OBJECT_ENV \
    "HOPLITE_HARA_BLOB_MAX_OBJECT_BYTES"
#define HOPLITE_HARA_BLOB_MAX_APPEND_ENV \
    "HOPLITE_HARA_BLOB_MAX_APPEND_BYTES"
#define HOPLITE_HARA_BLOB_MAX_SOURCE_CHUNK_ENV \
    "HOPLITE_HARA_BLOB_MAX_SOURCE_CHUNK_BYTES"
#define HOPLITE_HARA_BLOB_MAX_STAGING_KEY_ENV \
    "HOPLITE_HARA_BLOB_MAX_STAGING_KEY_BYTES"
#define HOPLITE_HARA_BLOB_MAX_MEDIA_TYPE_ENV \
    "HOPLITE_HARA_BLOB_MAX_MEDIA_TYPE_BYTES"
#define HOPLITE_HARA_BLOB_MAX_STAGING_ENTRIES_ENV \
    "HOPLITE_HARA_BLOB_MAX_STAGING_ENTRIES"
#define HOPLITE_HARA_BLOB_MAX_OBJECTS_ENV \
    "HOPLITE_HARA_BLOB_MAX_OBJECTS"

#define HOPLITE_HARA_BLOB_DEFAULT_MAX_OBJECT \
    (16u * 1024u * 1024u)
#define HOPLITE_HARA_BLOB_DEFAULT_MAX_APPEND \
    (1024u * 1024u)
#define HOPLITE_HARA_BLOB_DEFAULT_MAX_SOURCE_CHUNK \
    (64u * 1024u)
#define HOPLITE_HARA_BLOB_DEFAULT_MAX_STAGING_KEY 256u
#define HOPLITE_HARA_BLOB_DEFAULT_MAX_MEDIA_TYPE 256u
#define HOPLITE_HARA_BLOB_DEFAULT_MAX_STAGING_ENTRIES 1024u
#define HOPLITE_HARA_BLOB_DEFAULT_MAX_OBJECTS 65536u

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
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
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
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }
    if (result.data == NULL || result.len == 0
        || (result.kind != HOPLITE_BLOB_STORE_RESULT_SUCCESS
            && result.kind != HOPLITE_BLOB_STORE_RESULT_FAILURE))
    {
        hoplite_blob_store_provider_result_free_v1(&result);
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
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

static int
hoplite_blob_parse_u64(const char *name,
                       uint64_t default_value,
                       uint64_t maximum,
                       uint64_t *output)
{
    const char *value;
    const char *cursor;
    char *end;
    unsigned long long parsed;

    if (name == NULL || default_value == 0 || maximum == 0
        || default_value > maximum || output == NULL)
    {
        return 0;
    }

    value = getenv(name);
    if (value == NULL || value[0] == '\0') {
        *output = default_value;
        return 1;
    }
    for (cursor = value; *cursor != '\0'; cursor++) {
        if (*cursor < '0' || *cursor > '9') {
            return 0;
        }
    }

    errno = 0;
    end = NULL;
    parsed = strtoull(value, &end, 10);
    if (errno != 0 || end == value || end == NULL || *end != '\0'
        || parsed == 0 || parsed > (unsigned long long) maximum)
    {
        return 0;
    }

    *output = (uint64_t) parsed;
    return 1;
}

static int
hoplite_blob_parse_size(const char *name,
                        size_t default_value,
                        size_t *output)
{
    uint64_t parsed;

    if (output == NULL
        || !hoplite_blob_parse_u64(
            name,
            (uint64_t) default_value,
            (uint64_t) SIZE_MAX,
            &parsed))
    {
        return 0;
    }
    *output = (size_t) parsed;
    return 1;
}

static int
hoplite_blob_load_limits(hoplite_blob_store_limits_v1_t *limits)
{
    if (limits == NULL
        || !hoplite_blob_parse_u64(
            HOPLITE_HARA_BLOB_MAX_OBJECT_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_OBJECT,
            UINT64_MAX,
            &limits->max_object_bytes)
        || !hoplite_blob_parse_size(
            HOPLITE_HARA_BLOB_MAX_APPEND_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_APPEND,
            &limits->max_append_bytes)
        || !hoplite_blob_parse_size(
            HOPLITE_HARA_BLOB_MAX_SOURCE_CHUNK_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_SOURCE_CHUNK,
            &limits->max_source_chunk_bytes)
        || !hoplite_blob_parse_size(
            HOPLITE_HARA_BLOB_MAX_STAGING_KEY_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_STAGING_KEY,
            &limits->max_staging_key_bytes)
        || !hoplite_blob_parse_size(
            HOPLITE_HARA_BLOB_MAX_MEDIA_TYPE_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_MEDIA_TYPE,
            &limits->max_media_type_bytes)
        || !hoplite_blob_parse_size(
            HOPLITE_HARA_BLOB_MAX_STAGING_ENTRIES_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_STAGING_ENTRIES,
            &limits->max_staging_entries)
        || !hoplite_blob_parse_size(
            HOPLITE_HARA_BLOB_MAX_OBJECTS_ENV,
            HOPLITE_HARA_BLOB_DEFAULT_MAX_OBJECTS,
            &limits->max_objects))
    {
        return 0;
    }

    return limits->max_source_chunk_bytes <= limits->max_append_bytes
        && limits->max_append_bytes <= limits->max_object_bytes;
}

static int32_t
hoplite_blob_register_provider(const char *root,
                               const hoplite_blob_store_limits_v1_t *limits)
{
    hoplite_blob_store_provider_t *provider = NULL;
    int32_t status;

    if (hoplite_blob_state != HOPLITE_BLOB_HOST_NEW
        || limits == NULL
        || hoplite_blob_store_provider_abi_version()
            != HOPLITE_BLOB_STORE_PROVIDER_ABI_VERSION)
    {
        hoplite_blob_state = HOPLITE_BLOB_HOST_FAILED;
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }

    if (root != NULL && root[0] != '\0') {
        status = hoplite_blob_store_provider_open_filesystem_v1(
            (const uint8_t *) root,
            strlen(root),
            limits,
            &provider);
    } else {
        status = hoplite_blob_store_provider_open_memory_v1(
            limits,
            &provider);
    }
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

int32_t
hoplite_blob_host_provider_init_process_v1(void)
{
    const char *root;
    hoplite_blob_store_limits_v1_t limits;

    if (hoplite_blob_state == HOPLITE_BLOB_HOST_READY) {
        return HOPLITE_BLOB_HOST_PROVIDER_OK;
    }
    if (hoplite_blob_state != HOPLITE_BLOB_HOST_NEW) {
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }
    if (!hoplite_blob_load_limits(&limits)) {
        hoplite_blob_state = HOPLITE_BLOB_HOST_FAILED;
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }

    root = getenv(HOPLITE_HARA_BLOB_ROOT_ENV);
    return hoplite_blob_register_provider(root, &limits);
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
