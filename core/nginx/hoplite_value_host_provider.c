#include "hoplite_value_host_provider.h"

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "hoplite_host_provider.h"
#include "hoplite_value_provider.h"

#define HOPLITE_VALUE_PROVIDER_ENV "HOPLITE_VALUE_PROVIDER"
#define HOPLITE_VALUE_ROOT_ENV "HOPLITE_VALUE_ROOT"
#define HOPLITE_VALUE_MAX_BYTES_ENV "HOPLITE_VALUE_MAX_BYTES"

#define HOPLITE_VALUE_FILESYSTEM "filesystem"
#define HOPLITE_VALUE_MAX_MEDIA_TYPE_BYTES 256u
#define HOPLITE_VALUE_DEFAULT_IO_CHUNK_BYTES (64u * 1024u)

enum {
    HOPLITE_VALUE_HOST_NEW = 0,
    HOPLITE_VALUE_HOST_DISABLED = 1,
    HOPLITE_VALUE_HOST_READY = 2,
    HOPLITE_VALUE_HOST_FAILED = 3,
    HOPLITE_VALUE_HOST_CLOSED = 4
};

static hoplite_value_provider_t *hoplite_value_provider;
static int hoplite_value_state;

static int32_t
hoplite_value_host_invoke(const hoplite_host_call_v1_t *call)
{
    hoplite_value_result_v1_t result;
    hoplite_host_complete_v1_pt complete;
    int32_t status;
    int32_t completion_status;

    if (call == NULL
        || call->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION
        || hoplite_value_state != HOPLITE_VALUE_HOST_READY
        || hoplite_value_provider == NULL
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
    status = hoplite_value_provider_execute_v1(
        hoplite_value_provider,
        call->operation.data,
        call->operation.len,
        call->arguments_hta.data,
        call->arguments_hta.len,
        &result);
    if (status != HOPLITE_VALUE_PROVIDER_OK) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    if (result.data == NULL || result.len == 0
        || (result.kind != HOPLITE_VALUE_RESULT_SUCCESS
            && result.kind != HOPLITE_VALUE_RESULT_FAILURE))
    {
        hoplite_value_provider_result_free_v1(&result);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    complete = result.kind == HOPLITE_VALUE_RESULT_SUCCESS
        ? call->completer.succeed
        : call->completer.fail;
    completion_status = complete(
        call->completer.context,
        result.data,
        result.len);
    hoplite_value_provider_result_free_v1(&result);

    return completion_status == HOPLITE_HOST_PROVIDER_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static const uint8_t hoplite_value_service_name[] = "hara.value";

static const hoplite_host_provider_v1_t hoplite_value_host_provider = {
    HOPLITE_HOST_PROVIDER_ABI_VERSION,
    {
        hoplite_value_service_name,
        sizeof(hoplite_value_service_name) - 1
    },
    hoplite_value_host_invoke,
    NULL,
    0
};

static int
hoplite_value_parse_limit(const char *name, size_t *output)
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
        return 0;
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
hoplite_value_host_provider_register_filesystem_v1(
    const uint8_t *root,
    size_t root_len,
    size_t max_frame_bytes)
{
    hoplite_value_provider_t *provider = NULL;
    size_t io_chunk_bytes;
    int32_t status;

    if (hoplite_value_state != HOPLITE_VALUE_HOST_NEW) {
        return HOPLITE_VALUE_HOST_PROVIDER_ERROR;
    }
    if (root == NULL || root_len == 0 || max_frame_bytes == 0
        || hoplite_value_provider_abi_version()
            != HOPLITE_VALUE_PROVIDER_ABI_VERSION)
    {
        hoplite_value_state = HOPLITE_VALUE_HOST_FAILED;
        return HOPLITE_VALUE_HOST_PROVIDER_ERROR;
    }

    io_chunk_bytes = max_frame_bytes < HOPLITE_VALUE_DEFAULT_IO_CHUNK_BYTES
        ? max_frame_bytes
        : HOPLITE_VALUE_DEFAULT_IO_CHUNK_BYTES;
    status = hoplite_value_provider_open_filesystem_v1(
        root,
        root_len,
        max_frame_bytes,
        HOPLITE_VALUE_MAX_MEDIA_TYPE_BYTES,
        io_chunk_bytes,
        &provider);
    if (status != HOPLITE_VALUE_PROVIDER_OK || provider == NULL) {
        hoplite_value_state = HOPLITE_VALUE_HOST_FAILED;
        return HOPLITE_VALUE_HOST_PROVIDER_ERROR;
    }

    status = hoplite_host_provider_register_v1(&hoplite_value_host_provider);
    if (status != HOPLITE_HOST_PROVIDER_REGISTER_OK) {
        hoplite_value_provider_close_v1(provider);
        hoplite_value_state = HOPLITE_VALUE_HOST_FAILED;
        return HOPLITE_VALUE_HOST_PROVIDER_ERROR;
    }

    hoplite_value_provider = provider;
    hoplite_value_state = HOPLITE_VALUE_HOST_READY;
    return HOPLITE_VALUE_HOST_PROVIDER_OK;
}

int32_t
hoplite_value_host_provider_init_process_v1(void)
{
    const char *driver;
    const char *root;
    const char *maximum;
    size_t max_frame_bytes;
    int driver_present;
    int root_present;
    int maximum_present;

    if (hoplite_value_state == HOPLITE_VALUE_HOST_READY) {
        return HOPLITE_VALUE_HOST_PROVIDER_OK;
    }
    if (hoplite_value_state == HOPLITE_VALUE_HOST_DISABLED) {
        return HOPLITE_VALUE_HOST_PROVIDER_DISABLED;
    }
    if (hoplite_value_state != HOPLITE_VALUE_HOST_NEW) {
        return HOPLITE_VALUE_HOST_PROVIDER_ERROR;
    }

    driver = getenv(HOPLITE_VALUE_PROVIDER_ENV);
    root = getenv(HOPLITE_VALUE_ROOT_ENV);
    maximum = getenv(HOPLITE_VALUE_MAX_BYTES_ENV);
    driver_present = driver != NULL && driver[0] != '\0';
    root_present = root != NULL && root[0] != '\0';
    maximum_present = maximum != NULL && maximum[0] != '\0';

    if (!driver_present && !root_present && !maximum_present) {
        hoplite_value_state = HOPLITE_VALUE_HOST_DISABLED;
        return HOPLITE_VALUE_HOST_PROVIDER_DISABLED;
    }
    if (!driver_present || !root_present || !maximum_present
        || strcmp(driver, HOPLITE_VALUE_FILESYSTEM) != 0
        || !hoplite_value_parse_limit(
            HOPLITE_VALUE_MAX_BYTES_ENV,
            &max_frame_bytes))
    {
        hoplite_value_state = HOPLITE_VALUE_HOST_FAILED;
        return HOPLITE_VALUE_HOST_PROVIDER_ERROR;
    }

    return hoplite_value_host_provider_register_filesystem_v1(
        (const uint8_t *) root,
        strlen(root),
        max_frame_bytes);
}

void
hoplite_value_host_provider_exit_process_v1(void)
{
    if (hoplite_value_provider != NULL) {
        hoplite_value_provider_close_v1(hoplite_value_provider);
        hoplite_value_provider = NULL;
    }
    if (hoplite_value_state == HOPLITE_VALUE_HOST_READY) {
        hoplite_value_state = HOPLITE_VALUE_HOST_CLOSED;
    }
}
