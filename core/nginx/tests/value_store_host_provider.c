#define _POSIX_C_SOURCE 200809L

#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "hoplite_host_provider.h"
#include "hoplite_value_store_host_provider.h"
#include "hoplite_value_store_provider.h"

struct hoplite_value_store_provider {
    uint32_t marker;
};

static struct hoplite_value_store_provider fake_provider = {0x53544f52u};
static size_t open_count;
static size_t close_count;
static size_t execute_count;
static size_t result_free_count;
static size_t succeed_count;
static size_t fail_count;
static uint8_t completed[16];
static size_t completed_len;
static int32_t completion_result = HOPLITE_HOST_PROVIDER_OK;

uint32_t
hoplite_value_store_provider_abi_version(void)
{
    return HOPLITE_VALUE_STORE_PROVIDER_ABI_VERSION;
}

int32_t
hoplite_value_store_provider_open_sqlite_v1(
    const uint8_t *path,
    size_t path_len,
    size_t max_value_bytes,
    size_t max_receipt_bytes,
    hoplite_value_store_provider_t **provider)
{
    assert(path != NULL);
    assert(path_len == sizeof("fixture.db") - 1);
    assert(memcmp(path, "fixture.db", path_len) == 0);
    assert(max_value_bytes == 8192);
    assert(max_receipt_bytes == 2048);
    assert(provider != NULL);
    open_count++;
    *provider = &fake_provider;
    return HOPLITE_VALUE_STORE_PROVIDER_OK;
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
hoplite_value_store_provider_execute_v1(
    hoplite_value_store_provider_t *provider,
    const uint8_t *operation,
    size_t operation_len,
    const uint8_t *arguments_hta,
    size_t arguments_hta_len,
    hoplite_value_store_result_v1_t *result)
{
    static const uint8_t success[] = {0x48, 0x54, 0x41, 0x31, 0x01};
    static const uint8_t failure[] = {0x48, 0x54, 0x41, 0x31, 0x02};
    const uint8_t *source;
    size_t source_len;

    assert(provider == &fake_provider);
    assert(arguments_hta != NULL);
    assert(arguments_hta_len == 4);
    assert(memcmp(arguments_hta, "args", 4) == 0);
    assert(result != NULL);
    execute_count++;

    if (operation_is(operation, operation_len, "abi-error")) {
        return HOPLITE_VALUE_STORE_PROVIDER_INVALID;
    }
    if (operation_is(operation, operation_len, "invalid-kind")) {
        source = success;
        source_len = sizeof(success);
        result->kind = 99;
    } else if (operation_is(operation, operation_len, "receipt")) {
        source = failure;
        source_len = sizeof(failure);
        result->kind = HOPLITE_VALUE_STORE_RESULT_FAILURE;
    } else {
        assert(operation_is(operation, operation_len, "load"));
        source = success;
        source_len = sizeof(success);
        result->kind = HOPLITE_VALUE_STORE_RESULT_SUCCESS;
    }

    result->data = malloc(source_len);
    assert(result->data != NULL);
    memcpy(result->data, source, source_len);
    result->len = source_len;
    return HOPLITE_VALUE_STORE_PROVIDER_OK;
}

void
hoplite_value_store_provider_result_free_v1(
    hoplite_value_store_result_v1_t *result)
{
    if (result == NULL || result->data == NULL) {
        return;
    }
    result_free_count++;
    free(result->data);
    result->data = NULL;
    result->len = 0;
    result->kind = 0;
}

void
hoplite_value_store_provider_close_v1(
    hoplite_value_store_provider_t *provider)
{
    assert(provider == &fake_provider);
    close_count++;
}

static int32_t
complete_success(void *context, const uint8_t *hta, size_t hta_len)
{
    assert(context == &fake_provider);
    assert(hta != NULL);
    assert(hta_len <= sizeof(completed));
    succeed_count++;
    memcpy(completed, hta, hta_len);
    completed_len = hta_len;
    return completion_result;
}

static int32_t
complete_failure(void *context, const uint8_t *hta, size_t hta_len)
{
    assert(context == &fake_provider);
    assert(hta != NULL);
    assert(hta_len <= sizeof(completed));
    fail_count++;
    memcpy(completed, hta, hta_len);
    completed_len = hta_len;
    return completion_result;
}

static hoplite_host_call_v1_t
call_for(const char *operation)
{
    hoplite_host_call_v1_t call;

    memset(&call, 0, sizeof(call));
    call.abi_version = HOPLITE_HOST_PROVIDER_ABI_VERSION;
    call.request_context = &fake_provider;
    call.work = 7;
    call.call = 11;
    call.operation.data = (const uint8_t *) operation;
    call.operation.len = strlen(operation);
    call.arguments_hta.data = (const uint8_t *) "args";
    call.arguments_hta.len = 4;
    call.completer.context = &fake_provider;
    call.completer.succeed = complete_success;
    call.completer.fail = complete_failure;
    return call;
}

static void
check_disabled_configuration(void)
{
    hoplite_host_service_t service = {
        (const uint8_t *) "hara.store",
        sizeof("hara.store") - 1
    };

    unsetenv("HOPLITE_HARA_STORE_PATH");
    assert(hoplite_value_store_host_provider_init_process_v1()
           == HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED);
    assert(hoplite_value_store_host_provider_init_process_v1()
           == HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED);
    assert(hoplite_host_provider_find_v1(service) == NULL);
    hoplite_value_store_host_provider_exit_process_v1();
    assert(close_count == 0);
}

static void
check_invalid_environment(void)
{
    assert(setenv("HOPLITE_HARA_STORE_PATH", "fixture.db", 1) == 0);
    assert(setenv("HOPLITE_HARA_STORE_MAX_VALUE_BYTES", "0", 1) == 0);
    assert(hoplite_value_store_host_provider_init_process_v1()
           == HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR);
    assert(open_count == 0);
    hoplite_value_store_host_provider_exit_process_v1();
    assert(close_count == 0);
}

static void
check_registered_provider(void)
{
    hoplite_host_service_t service = {
        (const uint8_t *) "hara.store",
        sizeof("hara.store") - 1
    };
    const hoplite_host_provider_v1_t *provider;
    hoplite_host_call_v1_t call;
    size_t before;

    assert(hoplite_value_store_host_provider_register_sqlite_v1(
               (const uint8_t *) "fixture.db",
               sizeof("fixture.db") - 1,
               8192,
               2048)
           == HOPLITE_VALUE_STORE_HOST_PROVIDER_OK);
    assert(open_count == 1);
    assert(hoplite_value_store_host_provider_init_process_v1()
           == HOPLITE_VALUE_STORE_HOST_PROVIDER_OK);

    provider = hoplite_host_provider_find_v1(service);
    assert(provider != NULL);
    assert(provider->abi_version == HOPLITE_HOST_PROVIDER_ABI_VERSION);
    assert(provider->cancel == NULL);
    assert(provider->capabilities == 0);
    assert(provider->service.len == sizeof("hara.store") - 1);
    assert(memcmp(provider->service.data, "hara.store",
                  provider->service.len) == 0);

    call = call_for("load");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_OK);
    assert(succeed_count == 1);
    assert(fail_count == 0);
    assert(result_free_count == 1);
    assert(completed_len == 5);
    assert(completed[4] == 0x01);

    call = call_for("receipt");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_OK);
    assert(succeed_count == 1);
    assert(fail_count == 1);
    assert(result_free_count == 2);
    assert(completed_len == 5);
    assert(completed[4] == 0x02);

    before = result_free_count;
    call = call_for("invalid-kind");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_ERROR);
    assert(succeed_count == 1);
    assert(fail_count == 1);
    assert(result_free_count == before + 1);

    before = result_free_count;
    call = call_for("abi-error");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_ERROR);
    assert(result_free_count == before);

    completion_result = HOPLITE_HOST_PROVIDER_ERROR;
    call = call_for("load");
    assert(provider->invoke(&call) == HOPLITE_HOST_PROVIDER_ERROR);
    assert(succeed_count == 2);
    assert(result_free_count == before + 1);

    assert(execute_count == 5);
    hoplite_value_store_host_provider_exit_process_v1();
    hoplite_value_store_host_provider_exit_process_v1();
    assert(close_count == 1);
}

int
main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "disabled") == 0) {
        check_disabled_configuration();
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "invalid-env") == 0) {
        check_invalid_environment();
        return 0;
    }
    assert(argc == 1);
    check_registered_provider();
    return 0;
}
