#!/usr/bin/env python3
from pathlib import Path


def replace_exact(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new)


def patch_hta() -> None:
    path = Path("core/nginx/hoplite_hta.c")
    source = path.read_text()

    source = replace_exact(
        source,
        """    uint64_t raw;
    ngx_uint_t tag;

    if (hoplite_take(reader, 1, &tag_data) != NGX_OK) {
""",
        """    uint64_t raw;
    ngx_uint_t tag;
    size_t start = reader->cursor;

    if (hoplite_take(reader, 1, &tag_data) != NGX_OK) {
""",
        "record HTA value start",
    )

    source = replace_exact(
        source,
        """    if (value == NULL) {
        return NGX_ERROR;
    }
    *output = value;
    return NGX_OK;
}

ngx_int_t
hoplite_hta_decode""",
        """    if (value == NULL) {
        return NGX_ERROR;
    }
    value->encoded = reader->data + start;
    value->encoded_len = reader->cursor - start;
    *output = value;
    return NGX_OK;
}

ngx_int_t
hoplite_hta_decode""",
        "record HTA value span",
    )

    source = replace_exact(
        source,
        """static ngx_int_t
hoplite_write(hoplite_writer_t *writer, const void *data, size_t len)
""",
        """ngx_int_t
hoplite_hta_copy_frame(ngx_pool_t *pool,
                       const hoplite_hta_value_t *value,
                       ngx_str_t *output)
{
    if (pool == NULL || value == NULL || output == NULL
        || value->encoded == NULL || value->encoded_len == 0
        || value->encoded_len > (size_t) -1 - sizeof(hoplite_magic))
    {
        return NGX_ERROR;
    }

    output->len = sizeof(hoplite_magic) + value->encoded_len;
    output->data = ngx_pnalloc(pool, output->len);
    if (output->data == NULL) {
        output->len = 0;
        return NGX_ERROR;
    }
    ngx_memcpy(output->data, hoplite_magic, sizeof(hoplite_magic));
    ngx_memcpy(output->data + sizeof(hoplite_magic),
               value->encoded, value->encoded_len);
    return NGX_OK;
}

static ngx_int_t
hoplite_write(hoplite_writer_t *writer, const void *data, size_t len)
""",
        "add standalone HTA frame copy",
    )

    path.write_text(source)


def patch_module() -> None:
    path = Path("core/nginx/ngx_http_hoplite_module.c")
    source = path.read_text()

    source = replace_exact(
        source,
        """#include "hoplite_hta.h"
#include "hoplite_host_registry.h"
#include "hoplite_runtime.h"
""",
        """#include "hoplite_hta.h"
#include "hoplite_host_provider.h"
#include "hoplite_runtime.h"
""",
        "include exported provider ABI",
    )

    source = replace_exact(
        source,
        """typedef struct ngx_http_hoplite_ctx_s ngx_http_hoplite_ctx_t;

typedef struct {
""",
        """typedef struct ngx_http_hoplite_ctx_s ngx_http_hoplite_ctx_t;
typedef struct ngx_http_hoplite_native_completion_s
    ngx_http_hoplite_native_completion_t;

typedef struct {
""",
        "declare native completion type",
    )

    source = replace_exact(
        source,
        """    ngx_http_hoplite_sleep_t *sleep;
    const ngx_http_hoplite_provider_t *provider;
    ngx_http_hoplite_body_t body;
};

static int
""",
        """    ngx_http_hoplite_sleep_t *sleep;
    const ngx_http_hoplite_provider_t *provider;
    const hoplite_host_provider_v1_t *native_provider;
    ngx_http_hoplite_native_completion_t *native_completion;
    ngx_http_hoplite_body_t body;
};

struct ngx_http_hoplite_native_completion_s {
    ngx_http_hoplite_ctx_t *ctx;
    uint64_t call;
    ngx_log_t *log;
    ngx_flag_t completed;
    ngx_flag_t retained;
    ngx_flag_t failed;
};

static int
""",
        "extend request context for native providers",
    )

    source = replace_exact(
        source,
        """    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {
        ctx->provider->cancel(ctx);
    }
    ctx->provider = NULL;
    if (ctx->queued) {
""",
        """    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {
        ctx->provider->cancel(ctx);
    }
    if (ctx->native_provider != NULL
        && ctx->native_provider->cancel != NULL)
    {
        ctx->native_provider->cancel(ctx);
    }
    if (ctx->native_completion != NULL) {
        ctx->native_completion->completed = 1;
        ctx->native_completion = NULL;
    }
    ctx->provider = NULL;
    ctx->native_provider = NULL;
    if (ctx->queued) {
""",
        "cancel native provider on request finish",
    )

    provider_find = """static const ngx_http_hoplite_provider_t *
ngx_http_hoplite_provider_find(ngx_str_t service)
{
    hoplite_host_service_t lookup;
    lookup.data = service.data;
    lookup.len = service.len;
    return hoplite_host_registry_find(&ngx_http_hoplite_providers, lookup);
}

"""
    completion_code = provider_find + """static int32_t
ngx_http_hoplite_native_complete(void *data,
                                 const uint8_t *hta,
                                 size_t hta_len,
                                 ngx_flag_t failure)
{
    ngx_http_hoplite_native_completion_t *completion = data;
    ngx_http_hoplite_ctx_t *ctx;
    int rc;

    if (completion == NULL || completion->ctx == NULL
        || completion->completed || (hta_len != 0 && hta == NULL)
        || ngx_http_hoplite_runtime == NULL)
    {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ctx = completion->ctx;
    if (ctx->done || ctx->native_completion != completion) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }

    completion->completed = 1;
    ctx->native_completion = NULL;
    ctx->native_provider = NULL;
    rc = failure
        ? hoplite_call_reject(ngx_http_hoplite_runtime,
                              completion->call, hta, hta_len)
        : hoplite_call_resolve(ngx_http_hoplite_runtime,
                               completion->call, hta, hta_len);
    if (rc != 0) {
        completion->failed = 1;
        if (completion->retained) {
            ngx_str_t body = ngx_string("Hoplite runtime delivery failed\\n");
            ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR,
                                  &body, NULL);
        }
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (completion->retained
        && ngx_http_hoplite_drain(completion->log) != NGX_OK)
    {
        ngx_str_t body = ngx_string("Hoplite runtime delivery failed\\n");
        completion->failed = 1;
        ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR,
                              &body, NULL);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    return HOPLITE_HOST_PROVIDER_OK;
}

static int32_t
ngx_http_hoplite_native_succeed(void *data,
                                const uint8_t *hta,
                                size_t hta_len)
{
    return ngx_http_hoplite_native_complete(data, hta, hta_len, 0);
}

static int32_t
ngx_http_hoplite_native_fail(void *data,
                             const uint8_t *hta,
                             size_t hta_len)
{
    return ngx_http_hoplite_native_complete(data, hta, hta_len, 1);
}

"""
    source = replace_exact(
        source,
        provider_find,
        completion_code,
        "install native completion callbacks",
    )

    source = replace_exact(
        source,
        """    const ngx_http_hoplite_provider_t *provider;
    ngx_http_hoplite_host_call_t call;
    ngx_int_t rc;
""",
        """    const ngx_http_hoplite_provider_t *provider;
    const hoplite_host_provider_v1_t *native_provider;
    hoplite_host_service_t lookup;
    ngx_http_hoplite_host_call_t call;
    hoplite_host_call_v1_t native_call;
    ngx_http_hoplite_native_completion_t *completion;
    ngx_str_t operation_copy, arguments_hta;
    ngx_int_t rc;
    int32_t native_rc;
""",
        "declare native provider dispatch state",
    )

    source = replace_exact(
        source,
        """    if (ctx->provider != NULL) {
        return ngx_http_hoplite_reject(
            (uint64_t) call_number, pool,
            "request already has a pending Hoplite operation");
    }

    provider = ngx_http_hoplite_provider_find(service);
    if (provider == NULL) {
        return ngx_http_hoplite_reject((uint64_t) call_number, pool,
                                       "unsupported Hoplite host service");
    }

    call.ctx = ctx;
    call.work = (uint64_t) work_number;
    call.call = (uint64_t) call_number;
    call.operation = operation;
    call.arguments = event->as.vector.items[7];
    call.pool = pool;
    call.log = log;
    rc = provider->invoke(&call);
    if (rc == NGX_AGAIN) {
        ctx->provider = provider;
        return NGX_OK;
    }
    return rc;
}
""",
        """    if (ctx->provider != NULL || ctx->native_provider != NULL
        || ctx->native_completion != NULL)
    {
        return ngx_http_hoplite_reject(
            (uint64_t) call_number, pool,
            "request already has a pending Hoplite operation");
    }

    provider = ngx_http_hoplite_provider_find(service);
    if (provider != NULL) {
        call.ctx = ctx;
        call.work = (uint64_t) work_number;
        call.call = (uint64_t) call_number;
        call.operation = operation;
        call.arguments = event->as.vector.items[7];
        call.pool = pool;
        call.log = log;
        rc = provider->invoke(&call);
        if (rc == NGX_AGAIN) {
            ctx->provider = provider;
            return NGX_OK;
        }
        return rc;
    }

    lookup.data = service.data;
    lookup.len = service.len;
    native_provider = hoplite_host_provider_find_v1(lookup);
    if (native_provider == NULL) {
        return ngx_http_hoplite_reject((uint64_t) call_number, pool,
                                       "unsupported Hoplite host service");
    }
    if (ngx_http_hoplite_copy(ctx->request->pool,
                              &operation, &operation_copy) != NGX_OK
        || hoplite_hta_copy_frame(ctx->request->pool,
                                  event->as.vector.items[7],
                                  &arguments_hta) != NGX_OK)
    {
        return NGX_ERROR;
    }

    completion = ngx_pcalloc(ctx->request->pool, sizeof(*completion));
    if (completion == NULL) {
        return NGX_ERROR;
    }
    completion->ctx = ctx;
    completion->call = (uint64_t) call_number;
    completion->log = log;
    ctx->native_completion = completion;

    native_call.abi_version = HOPLITE_HOST_PROVIDER_ABI_VERSION;
    native_call.request_context = ctx;
    native_call.work = (uint64_t) work_number;
    native_call.call = (uint64_t) call_number;
    native_call.operation.data = operation_copy.data;
    native_call.operation.len = operation_copy.len;
    native_call.arguments_hta.data = arguments_hta.data;
    native_call.arguments_hta.len = arguments_hta.len;
    native_call.completer.context = completion;
    native_call.completer.succeed = ngx_http_hoplite_native_succeed;
    native_call.completer.fail = ngx_http_hoplite_native_fail;

    native_rc = native_provider->invoke(&native_call);
    if (completion->completed) {
        return completion->failed ? NGX_ERROR : NGX_OK;
    }
    if (native_rc == HOPLITE_HOST_PROVIDER_PENDING) {
        completion->retained = 1;
        ctx->native_provider = native_provider;
        return NGX_OK;
    }

    completion->completed = 1;
    ctx->native_completion = NULL;
    if (native_rc == HOPLITE_HOST_PROVIDER_OK) {
        return ngx_http_hoplite_reject(
            (uint64_t) call_number, pool,
            "native provider returned without completing");
    }
    return NGX_ERROR;
}
""",
        "dispatch built-in and exported providers",
    )

    source = replace_exact(
        source,
        """    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {
        ctx->provider->cancel(ctx);
    }
    ctx->provider = NULL;

    if (!ctx->done && ngx_http_hoplite_runtime != NULL && ctx->work != 0) {
""",
        """    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {
        ctx->provider->cancel(ctx);
    }
    if (ctx->native_provider != NULL
        && ctx->native_provider->cancel != NULL)
    {
        ctx->native_provider->cancel(ctx);
    }
    if (ctx->native_completion != NULL) {
        ctx->native_completion->completed = 1;
        ctx->native_completion = NULL;
    }
    ctx->provider = NULL;
    ctx->native_provider = NULL;

    if (!ctx->done && ngx_http_hoplite_runtime != NULL && ctx->work != 0) {
""",
        "cancel native provider on cleanup",
    )

    path.write_text(source)


def patch_config() -> None:
    path = Path("core/nginx/config")
    source = path.read_text()
    source = replace_exact(
        source,
        """ngx_module_srcs="$ngx_addon_dir/ngx_http_hoplite_module.c \\
                 $ngx_addon_dir/hoplite_hta.c"
""",
        """ngx_module_srcs="$ngx_addon_dir/ngx_http_hoplite_module.c \\
                 $ngx_addon_dir/hoplite_hta.c \\
                 $ngx_addon_dir/hoplite_host_provider.c"
""",
        "link native provider registry",
    )
    path.write_text(source)


def main() -> None:
    patch_hta()
    patch_module()
    patch_config()


if __name__ == "__main__":
    main()
