#include "hoplite_value_store_host_provider.h"

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "hoplite_host_provider.h"
#include "hoplite_value_host_provider.h"
#include "hoplite_value_store_provider.h"

#define HOPLITE_STORE_PATH_ENV "HOPLITE_STORE_PATH"
#define HOPLITE_STORE_MAX_VALUE_ENV \
    "HOPLITE_STORE_MAX_VALUE_BYTES"
#define HOPLITE_STORE_MAX_RECEIPT_ENV \
    "HOPLITE_STORE_MAX_RECEIPT_BYTES"

#define HOPLITE_STORE_DEFAULT_MAX_VALUE (8u * 1024u * 1024u)
#define HOPLITE_STORE_DEFAULT_MAX_RECEIPT (1024u * 1024u)

enum {
    HOPLITE_VALUE_STORE_HOST_NEW = 0,
    HOPLITE_VALUE_STORE_HOST_DISABLED = 1,
    HOPLITE_VALUE_STORE_HOST_READY = 2,
    HOPLITE_VALUE_STORE_HOST_FAILED = 3,
    HOPLITE_VALUE_STORE_HOST_CLOSED = 4
};

static hoplite_value_store_provider_t *hoplite_value_store_provider;
static int hoplite_value_store_state;

static int32_t
hoplite_value_store_host_invoke(const hoplite_host_call_v1_t *call)
{
    hoplite_value_store_result_v1_t result;
    hoplite_host_complete_v1_pt complete;
    int32_t status;
    int32_t completion_status;

    if (call == NULL
        || call->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION
        || hoplite_value_store_state != HOPLITE_VALUE_STORE_HOST_READY
        || hoplite_value_store_provider == NULL
        || call->operation.data == NULL
        || call->operation.len == 0
        || call->arguments_hta.data == NULL
        || call->arguments_hta.len == 0
        || call->completer.succeed == NULL
        || call->completer.fail == NULL)
    {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    memset(&result, 0, sizeof(result));
    status = hoplite_value_store_provider_execute_v1(
        hoplite_value_store_provider,
        call->operation.data,
        call->operation.len,
        call->arguments_hta.data,
        call->arguments_hta.len,
        &result);
    if (status != HOPLITE_VALUE_STORE_PROVIDER_OK) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    if (result.data == NULL || result.len == 0
        || (result.kind != HOPLITE_VALUE_STORE_RESULT_SUCCESS
            && result.kind != HOPLITE_VALUE_STORE_RESULT_FAILURE))
    {
        hoplite_value_store_provider_result_free_v1(&result);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    complete = result.kind == HOPLITE_VALUE_STORE_RESULT_SUCCESS
        ? call->completer.succeed
        : call->completer.fail;
    completion_status = complete(
        call->completer.context,
        result.data,
        result.len);
    hoplite_value_store_provider_result_free_v1(&result);

    return completion_status == HOPLITE_HOST_PROVIDER_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static const uint8_t hoplite_value_store_service_name[] = "hoplite.store";

static const hoplite_host_provider_v1_t hoplite_value_store_host_provider = {
    HOPLITE_HOST_PROVIDER_ABI_VERSION,
    {
        hoplite_value_store_service_name,
        sizeof(hoplite_value_store_service_name) - 1
    },
    hoplite_value_store_host_invoke,
    NULL,
    0,
    NULL,
    NULL,
    NULL
};

static int
hoplite_value_store_parse_limit(const char *name,
                                size_t default_value,
                                size_t *output)
{
    const char *value;
    const char *cursor;
    char *end;
    unsigned long long parsed;

    if (output == NULL) {
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
        || parsed == 0 || parsed > (unsigned long long) SIZE_MAX)
    {
        return 0;
    }
    *output = (size_t) parsed;
    return 1;
}

int32_t
hoplite_value_store_host_provider_register_sqlite_v1(
    const uint8_t *path,
    size_t path_len,
    size_t max_value_bytes,
    size_t max_receipt_bytes)
{
    hoplite_value_store_provider_t *provider = NULL;
    int32_t status;

    if (hoplite_value_store_state != HOPLITE_VALUE_STORE_HOST_NEW) {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }
    if (path == NULL || path_len == 0
        || max_value_bytes == 0 || max_receipt_bytes == 0
        || hoplite_value_store_provider_abi_version()
            != HOPLITE_VALUE_STORE_PROVIDER_ABI_VERSION)
    {
        hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_FAILED;
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    status = hoplite_value_store_provider_open_sqlite_v1(
        path,
        path_len,
        max_value_bytes,
        max_receipt_bytes,
        &provider);
    if (status != HOPLITE_VALUE_STORE_PROVIDER_OK || provider == NULL) {
        hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_FAILED;
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    status = hoplite_host_provider_register_v1(
        &hoplite_value_store_host_provider);
    if (status != HOPLITE_HOST_PROVIDER_REGISTER_OK) {
        hoplite_value_store_provider_close_v1(provider);
        hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_FAILED;
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    hoplite_value_store_provider = provider;
    hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_READY;
    return HOPLITE_VALUE_STORE_HOST_PROVIDER_OK;
}

static int32_t
hoplite_value_store_host_provider_init_store_v1(void)
{
    const char *path;
    size_t max_value_bytes;
    size_t max_receipt_bytes;

    if (hoplite_value_store_state == HOPLITE_VALUE_STORE_HOST_READY) {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_OK;
    }
    if (hoplite_value_store_state == HOPLITE_VALUE_STORE_HOST_DISABLED) {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED;
    }
    if (hoplite_value_store_state != HOPLITE_VALUE_STORE_HOST_NEW) {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    path = getenv(HOPLITE_STORE_PATH_ENV);
    if (path == NULL || path[0] == '\0') {
        hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_DISABLED;
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED;
    }
    if (!hoplite_value_store_parse_limit(
            HOPLITE_STORE_MAX_VALUE_ENV,
            HOPLITE_STORE_DEFAULT_MAX_VALUE,
            &max_value_bytes)
        || !hoplite_value_store_parse_limit(
            HOPLITE_STORE_MAX_RECEIPT_ENV,
            HOPLITE_STORE_DEFAULT_MAX_RECEIPT,
            &max_receipt_bytes))
    {
        hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_FAILED;
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    return hoplite_value_store_host_provider_register_sqlite_v1(
        (const uint8_t *) path,
        strlen(path),
        max_value_bytes,
        max_receipt_bytes);
}

static void
hoplite_value_store_host_provider_exit_store_v1(void)
{
    if (hoplite_value_store_provider != NULL) {
        hoplite_value_store_provider_close_v1(hoplite_value_store_provider);
        hoplite_value_store_provider = NULL;
    }
    if (hoplite_value_store_state == HOPLITE_VALUE_STORE_HOST_READY) {
        hoplite_value_store_state = HOPLITE_VALUE_STORE_HOST_CLOSED;
    }
}

/*
 * This existing worker hook is the installed-provider bootstrap aggregator.
 * The providers retain separate configuration, handles and registry entries.
 */
int32_t
hoplite_value_store_host_provider_init_process_v1(void)
{
    int32_t store_status;
    int32_t value_status;

    store_status = hoplite_value_store_host_provider_init_store_v1();
    if (store_status == HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR) {
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    value_status = hoplite_value_host_provider_init_process_v1();
    if (value_status == HOPLITE_VALUE_HOST_PROVIDER_ERROR) {
        hoplite_value_host_provider_exit_process_v1();
        hoplite_value_store_host_provider_exit_store_v1();
        return HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR;
    }

    if (store_status == HOPLITE_VALUE_STORE_HOST_PROVIDER_DISABLED
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
    hoplite_value_store_host_provider_exit_store_v1();
}
