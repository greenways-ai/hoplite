#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "hoplite_blob_host_provider.h"
#include "hoplite_blob_store_provider.h"
#include "hoplite_host_provider.h"

#define HOPLITE_HARA_BLOB_ROOT_ENV "HOPLITE_HARA_BLOB_ROOT"
#define HOPLITE_BLOB_TEST_EXPECT_INIT_FAILURE \
    "HOPLITE_BLOB_TEST_EXPECT_INIT_FAILURE"

struct hoplite_blob_store_provider {
    uint32_t marker;
};

static struct hoplite_blob_store_provider fake_provider = {0x424c4f42u};
static size_t memory_open_count;
static size_t filesystem_open_count;
static size_t close_count;
static size_t execute_count;
static size_t free_count;
static size_t release_count;
static size_t succeed_count;
static size_t fail_count;
static uint64_t released_work;
static int filesystem_expected;
static int32_t completion_result = HOPLITE_HOST_PROVIDER_OK;

static unsigned long long
expected_limit(const char *name, unsigned long long default_value)
{
    const char *value = getenv(name);
    char *end = NULL;
    unsigned long long parsed;

    if (value == NULL || value[0] == '\0') {
        return default_value;
    }
    parsed = strtoull(value, &end, 10);
    assert(end != value);
    assert(end != NULL);
    assert(*end == '\0');
    assert(parsed != 0);
    return parsed;
}

static void
assert_limits(const hoplite_blob_store_limits_v1_t *limits)
{
    assert(limits != NULL);
    assert(limits->max_object_bytes == expected_limit(
        "HOPLITE_HARA_BLOB_MAX_OBJECT_BYTES",
        16u * 1024u * 1024u));
    assert(limits->max_append_bytes == (size_t) expected_limit(
        "HOPLITE_HARA_BLOB_MAX_APPEND_BYTES",
        1024u * 1024u));
    assert(limits->max_source_chunk_bytes == (size_t) expected_limit(
        "HOPLITE_HARA_BLOB_MAX_SOURCE_CHUNK_BYTES",
        64u * 1024u));
    assert(limits->max_staging_key_bytes == (size_t) expected_limit(
        "HOPLITE_HARA_BLOB_MAX_STAGING_KEY_BYTES",
        256u));
    assert(limits->max_media_type_bytes == (size_t) expected_limit(
        "HOPLITE_HARA_BLOB_MAX_MEDIA_TYPE_BYTES",
        256u));
    assert(limits->max_staging_entries == (size_t) expected_limit(
        "HOPLITE_HARA_BLOB_MAX_STAGING_ENTRIES",
        1024u));
    assert(limits->max_objects == (size_t) expected_limit(
        "HOPLITE_HARA_BLOB_MAX_OBJECTS",
        65536u));
}

uint32_t
hoplite_blob_store_provider_abi_version(void)
{
    return HOPLITE_BLOB_STORE_PROVIDER_ABI_VERSION;
}

int32_t
hoplite_blob_store_provider_open_memory_v1(
    const hoplite_blob_store_limits_v1_t *limits,
    hoplite_blob_store_provider_t **provider)
{
    assert(!filesystem_expected);
    assert_limits(limits);
    assert(provider != NULL);
    memory_open_count++;
    *provider = &fake_provider;
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

int32_t
hoplite_blob_store_provider_open_filesystem_v1(
    const uint8_t *root,
    size_t root_len,
    const hoplite_blob_store_limits_v1_t *limits,
    hoplite_blob_store_provider_t **provider)
{
    const char *expected = getenv(HOPLITE_HARA_BLOB_ROOT_ENV);

    assert(filesystem_expected);
    assert(expected != NULL);
    assert(root != NULL);
    assert(root_len == strlen(expected));
    assert(memcmp(root, expected, root_len) == 0);
    assert_limits(limits);
    assert(provider != NULL);
    filesystem_open_count++;
    *provider = &fake_provider;
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

int32_t
hoplite_host_request_body_read_v1(
    void *request_context,
    uint64_t work,
    uint64_t handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned)
{
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(handle == 19);
    assert(output != NULL);
    assert(capacity != 0);
    assert(returned != NULL);
    *returned = 0;
    return HOPLITE_HOST_RESOURCE_OK;
}

int32_t
hoplite_host_request_body_finish_v1(
    void *request_context,
    uint64_t work,
    uint64_t handle)
{
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(handle == 19);
    return HOPLITE_HOST_RESOURCE_OK;
}

static int
operation_is(const uint8_t *operation, size_t operation_len,
             const char *expected)
{
    size_t expected_len = strlen(expected);
    return operation_len == expected_len
        && memcmp(operation, expected, expected_len) == 0;
}

int32_t
hoplite_blob_store_provider_execute_v1(
    hoplite_blob_store_provider_t *provider,
    const hoplite_blob_store_call_v1_t *call,
    const uint8_t *operation,
    size_t operation_len,
    const uint8_t *arguments_hta,
    size_t arguments_hta_len,
    hoplite_blob_store_result_v1_t *result)
{
    static const uint8_t success[] = {0x48, 0x54, 0x41, 0x31, 0x01};
    static const uint8_t failure[] = {0x48, 0x54, 0x41, 0x31, 0x02};
    const uint8_t *source;
    size_t source_len;

    assert(provider == &fake_provider);
    assert(call != NULL);
    assert(call->abi_version == HOPLITE_BLOB_STORE_PROVIDER_ABI_VERSION);
    assert(call->request_context == &fake_provider);
    assert(call->work == 71);
    assert(call->request_read == hoplite_host_request_body_read_v1);
    assert(call->request_finish == hoplite_host_request_body_finish_v1);
    assert(arguments_hta != NULL);
    assert(arguments_hta_len == 4);
    assert(memcmp(arguments_hta, "args", 4) == 0);
    assert(result != NULL);
    execute_count++;

    if (operation_is(operation, operation_len, "ffi-error")) {
        return HOPLITE_BLOB_STORE_PROVIDER_FAILURE;
    }
    if (operation_is(operation, operation_len, "invalid-kind")) {
        source = success;
        source_len = sizeof(success);
        result->kind = 99;
    } else if (operation_is(operation, operation_len, "staging/abort")) {
        source = failure;
        source_len = sizeof(failure);
        result->kind = HOPLITE_BLOB_STORE_RESULT_FAILURE;
    } else {
        assert(operation_is(operation, operation_len, "staging/open"));
        source = success;
        source_len = sizeof(success);
        result->kind = HOPLITE_BLOB_STORE_RESULT_SUCCESS;
    }

    result->data = malloc(source_len);
    assert(result->data != NULL);
    memcpy(result->data, source, source_len);
    result->len = source_len;
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

int32_t
hoplite_blob_store_provider_response_read_scoped_v1(
    hoplite_blob_store_provider_t *provider,
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned)
{
    static const uint8_t source[] = "source";
    size_t amount;

    assert(provider == &fake_provider);
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(source_handle == 19);
    assert(output != NULL);
    assert(returned != NULL);
    amount = capacity < sizeof(source) - 1 ? capacity : sizeof(source) - 1;
    memcpy(output, source, amount);
    *returned = amount;
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

int32_t
hoplite_blob_store_provider_response_close_scoped_v1(
    hoplite_blob_store_provider_t *provider,
    void *request_context,
    uint64_t work,
    uint64_t source_handle)
{
    assert(provider == &fake_provider);
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(source_handle == 19);
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

size_t
hoplite_blob_store_provider_release_work_v1(
    hoplite_blob_store_provider_t *provider,
    uint64_t work)
{
    assert(provider == &fake_provider);
    released_work = work;
    release_count++;
    return work == 71 ? 2 : 0;
}

void
hoplite_blob_store_provider_result_free_v1(
    hoplite_blob_store_result_v1_t *result)
{
    if (result == NULL || result->data == NULL) {
        return;
    }
    free_count++;
    free(result->data);
    result->data = NULL;
    result->len = 0;
    result->kind = 0;
}

void
hoplite_blob_store_provider_close_v1(
    hoplite_blob_store_provider_t *provider)
{
    assert(provider == &fake_provider);
    close_count++;
}

static int32_t
complete_success(void *context, const uint8_t *hta, size_t hta_len)
{
    assert(context == &fake_provider);
    assert(hta != NULL);
    assert(hta_len == 5);
    succeed_count++;
    return completion_result;
}

static int32_t
complete_failure(void *context, const uint8_t *hta, size_t hta_len)
{
    assert(context == &fake_provider);
    assert(hta != NULL);
    assert(hta_len == 5);
    fail_count++;
    return completion_result;
}

static hoplite_host_call_v1_t
call_for(const char *operation)
{
    hoplite_host_call_v1_t call;

    memset(&call, 0, sizeof(call));
    call.abi_version = HOPLITE_HOST_PROVIDER_ABI_VERSION;
    call.request_context = &fake_provider;
    call.work = 71;
    call.call = 83;
    call.operation.data = (const uint8_t *) operation;
    call.operation.len = strlen(operation);
    call.arguments_hta.data = (const uint8_t *) "args";
    call.arguments_hta.len = 4;
    call.completer.context = &fake_provider;
    call.completer.succeed = complete_success;
    call.completer.fail = complete_failure;
    return call;
}

int
main(void)
{
    hoplite_host_service_t service = {
        (const uint8_t *) "hara.blob",
        sizeof("hara.blob") - 1
    };
    const hoplite_host_provider_v1_t *provider;
    hoplite_host_call_v1_t call;
    const char *root = getenv(HOPLITE_HARA_BLOB_ROOT_ENV);
    size_t before;

    filesystem_expected = root != NULL && root[0] != '\0';
    if (getenv(HOPLITE_BLOB_TEST_EXPECT_INIT_FAILURE) != NULL) {
        assert(hoplite_blob_host_provider_init_process_v1()
               == HOPLITE_BLOB_HOST_PROVIDER_ERROR);
        assert(memory_open_count == 0);
        assert(filesystem_open_count == 0);
        assert(hoplite_host_provider_find_v1(service) == NULL);
        hoplite_blob_host_provider_exit_process_v1();
        return 0;
    }

    assert(hoplite_blob_host_provider_init_process_v1()
           == HOPLITE_BLOB_HOST_PROVIDER_OK);
    assert(hoplite_blob_host_provider_init_process_v1()
           == HOPLITE_BLOB_HOST_PROVIDER_OK);
    assert(memory_open_count == (filesystem_expected ? 0u : 1u));
    assert(filesystem_open_count == (filesystem_expected ? 1u : 0u));

    provider = hoplite_host_provider_find_v1(service);
    assert(provider != NULL);
    assert(provider->abi_version == HOPLITE_HOST_PROVIDER_ABI_VERSION);
    assert(provider->cancel == NULL);
    assert(provider->capabilities
           == (HOPLITE_HOST_PROVIDER_REQUEST_BODY
               | HOPLITE_HOST_PROVIDER_RESPONSE_BODY));
    assert(provider->service.len == sizeof("hara.blob") - 1);
    assert(memcmp(provider->service.data, "hara.blob",
                  provider->service.len) == 0);

    call = call_for("staging/open");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_OK);
    assert(succeed_count == 1);
    assert(fail_count == 0);
    assert(free_count == 1);

    call = call_for("staging/abort");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_OK);
    assert(succeed_count == 1);
    assert(fail_count == 1);
    assert(free_count == 2);

    before = free_count;
    call = call_for("invalid-kind");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_ERROR);
    assert(free_count == before + 1);

    before = free_count;
    call = call_for("ffi-error");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_ERROR);
    assert(free_count == before);

    completion_result = HOPLITE_HOST_PROVIDER_ERROR;
    call = call_for("staging/open");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_ERROR);
    assert(succeed_count == 2);
    assert(free_count == before + 1);

    assert(hoplite_blob_host_provider_release_work_v1(71) == 2);
    assert(release_count == 1);
    assert(released_work == 71);
    assert(hoplite_blob_host_provider_release_work_v1(0) == 0);
    assert(release_count == 1);

    {
        uint8_t output[8];
        size_t returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   &fake_provider, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_OK);
        assert(returned == sizeof("source") - 1);
        assert(memcmp(output, "source", returned) == 0);
        returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   NULL, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_ERROR);
        assert(returned == 0);
        assert(hoplite_blob_host_provider_response_close_v1(
                   &fake_provider, 71, 19)
               == HOPLITE_BLOB_HOST_PROVIDER_OK);
    }

    assert(execute_count == 5);
    hoplite_blob_host_provider_exit_process_v1();
    hoplite_blob_host_provider_exit_process_v1();
    assert(close_count == 1);
    return 0;
}
