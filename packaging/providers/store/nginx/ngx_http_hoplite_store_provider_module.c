#include <ngx_config.h>
#include <ngx_core.h>
#include <ngx_http.h>

#include "hoplite_store_host_provider.h"

static ngx_int_t
ngx_http_hoplite_store_provider_init_process(ngx_cycle_t *cycle)
{
    int32_t status;

    status = hoplite_store_host_provider_init_process_v1();
    if (status != HOPLITE_STORE_HOST_PROVIDER_OK) {
        ngx_log_error(
            NGX_LOG_EMERG,
            cycle->log,
            0,
            status == HOPLITE_STORE_HOST_PROVIDER_DISABLED
                ? "hoplite.store provider requires HOPLITE_STORE_PATH"
                : "hoplite.store provider could not be initialized");
        return NGX_ERROR;
    }

    return NGX_OK;
}

static void
ngx_http_hoplite_store_provider_exit_process(ngx_cycle_t *cycle)
{
    (void) cycle;
    hoplite_store_host_provider_exit_process_v1();
}

static ngx_command_t ngx_http_hoplite_store_provider_commands[] = {
    ngx_null_command
};

static ngx_http_module_t ngx_http_hoplite_store_provider_module_ctx = {
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
};

ngx_module_t ngx_http_hoplite_store_provider_module = {
    NGX_MODULE_V1,
    &ngx_http_hoplite_store_provider_module_ctx,
    ngx_http_hoplite_store_provider_commands,
    NGX_HTTP_MODULE,
    NULL,
    NULL,
    ngx_http_hoplite_store_provider_init_process,
    NULL,
    NULL,
    ngx_http_hoplite_store_provider_exit_process,
    NULL,
    NGX_MODULE_V1_PADDING
};
