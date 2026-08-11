#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include "hoplite_host_provider.h"
#include "hoplite_store_host_provider.h"

static const hoplite_host_provider_v1_t *registered_provider;
static size_t register_count;

int32_t
hoplite_host_provider_register_v1(
    const hoplite_host_provider_v1_t *provider)
{
    if (provider == NULL || registered_provider != NULL) {
        return HOPLITE_HOST_PROVIDER_REGISTER_DUPLICATE;
    }
    registered_provider = provider;
    register_count++;
    return HOPLITE_HOST_PROVIDER_REGISTER_OK;
}

static int
service_equals(const hoplite_host_service_t *service, const char *expected)
{
    size_t length;

    if (service == NULL || expected == NULL || service->data == NULL) {
        return 0;
    }
    length = strlen(expected);
    return service->len == length
        && memcmp(service->data, expected, length) == 0;
}

static int
expect_status(int32_t actual, int32_t expected, const char *context)
{
    if (actual == expected) {
        return 1;
    }
    fprintf(
        stderr,
        "%s returned %d instead of %d\n",
        context,
        (int) actual,
        (int) expected);
    return 0;
}

static int
run_disabled(void)
{
    if (getenv("HOPLITE_STORE_PATH") != NULL) {
        fputs("disabled fixture inherited HOPLITE_STORE_PATH\n", stderr);
        return 1;
    }
    if (!expect_status(
            hoplite_store_host_provider_init_process_v1(),
            HOPLITE_STORE_HOST_PROVIDER_DISABLED,
            "disabled init"))
    {
        return 1;
    }
    if (register_count != 0 || registered_provider != NULL) {
        fputs("disabled fixture registered a provider\n", stderr);
        return 1;
    }
    if (!expect_status(
            hoplite_store_host_provider_init_process_v1(),
            HOPLITE_STORE_HOST_PROVIDER_DISABLED,
            "replayed disabled init"))
    {
        return 1;
    }
    hoplite_store_host_provider_exit_process_v1();
    hoplite_store_host_provider_exit_process_v1();
    return 0;
}

static int
run_invalid(void)
{
    if (getenv("HOPLITE_STORE_PATH") == NULL) {
        fputs("invalid fixture requires HOPLITE_STORE_PATH\n", stderr);
        return 1;
    }
    if (!expect_status(
            hoplite_store_host_provider_init_process_v1(),
            HOPLITE_STORE_HOST_PROVIDER_ERROR,
            "invalid init"))
    {
        return 1;
    }
    if (register_count != 0 || registered_provider != NULL) {
        fputs("invalid fixture registered a provider\n", stderr);
        return 1;
    }
    if (!expect_status(
            hoplite_store_host_provider_init_process_v1(),
            HOPLITE_STORE_HOST_PROVIDER_ERROR,
            "replayed invalid init"))
    {
        return 1;
    }
    hoplite_store_host_provider_exit_process_v1();
    hoplite_store_host_provider_exit_process_v1();
    return 0;
}

static int
run_ready(void)
{
    const char *path;
    struct stat metadata;

    path = getenv("HOPLITE_STORE_PATH");
    if (path == NULL || path[0] == '\0') {
        fputs("ready fixture requires HOPLITE_STORE_PATH\n", stderr);
        return 1;
    }
    if (!expect_status(
            hoplite_store_host_provider_init_process_v1(),
            HOPLITE_STORE_HOST_PROVIDER_OK,
            "ready init"))
    {
        return 1;
    }
    if (register_count != 1 || registered_provider == NULL) {
        fputs("ready fixture did not register exactly one provider\n", stderr);
        return 1;
    }
    if (registered_provider->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION
        || !service_equals(&registered_provider->service, "hoplite.store")
        || registered_provider->invoke == NULL
        || registered_provider->cancel != NULL
        || registered_provider->capabilities != 0
        || registered_provider->response_read != NULL
        || registered_provider->response_close != NULL
        || registered_provider->release_work != NULL)
    {
        fputs("ready fixture registered an invalid descriptor\n", stderr);
        return 1;
    }
    if (!expect_status(
            hoplite_store_host_provider_init_process_v1(),
            HOPLITE_STORE_HOST_PROVIDER_OK,
            "replayed ready init"))
    {
        return 1;
    }
    if (register_count != 1) {
        fputs("replayed ready init registered twice\n", stderr);
        return 1;
    }
    if (stat(path, &metadata) != 0 || !S_ISREG(metadata.st_mode)) {
        fputs("ready fixture did not create the SQLite store\n", stderr);
        return 1;
    }

    hoplite_store_host_provider_exit_process_v1();
    hoplite_store_host_provider_exit_process_v1();
    return 0;
}

int
main(int argc, char **argv)
{
    if (argc != 2) {
        fputs("usage: store_lifecycle disabled|invalid|ready\n", stderr);
        return 2;
    }
    if (strcmp(argv[1], "disabled") == 0) {
        return run_disabled();
    }
    if (strcmp(argv[1], "invalid") == 0) {
        return run_invalid();
    }
    if (strcmp(argv[1], "ready") == 0) {
        return run_ready();
    }
    fprintf(stderr, "unknown lifecycle fixture: %s\n", argv[1]);
    return 2;
}
