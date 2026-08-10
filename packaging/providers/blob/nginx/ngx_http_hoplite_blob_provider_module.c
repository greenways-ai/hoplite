#include <ngx_config.h>
#include <ngx_core.h>
#include <ngx_http.h>

#include "hoplite_blob_host_provider.h"

static ngx_int_t
ngx_http_hoplite_blob_provider_init_process(ngx_cycle_t *cycle)
{
    if (hoplite_blob_host_provider_init_process_v1()
        != HOPLITE_BLOB_HOST_PROVIDER_OK)
    {
        ngx_log_error(
            NGX_LOG_EMERG,
            cycle->log,
            0,
            "hoplite.blob provider could not be initialized");
        return NGX_ERROR;
    }

    return NGX_OK;
}

static void
ngx_http_hoplite_blob_provider_exit_process(ngx_cycle_t *cycle)
{
    (void) cycle;
    hoplite_blob_host_provider_exit_process_v1();
}

static ngx_command_t ngx_http_hoplite_blob_provider_commands[] = {
    ngx_null_command
};

static ngx_http_module_t ngx_http_hoplite_blob_provider_module_ctx = {
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
};

ngx_module_t ngx_http_hoplite_blob_provider_module = {
    NGX_MODULE_V1,
    &ngx_http_hoplite_blob_provider_module_ctx,
    ngx_http_hoplite_blob_provider_commands,
    NGX_HTTP_MODULE,
    NULL,
    NULL,
    ngx_http_hoplite_blob_provider_init_process,
    NULL,
    NULL,
    ngx_http_hoplite_blob_provider_exit_process,
    NULL,
    NGX_MODULE_V1_PADDING
};
