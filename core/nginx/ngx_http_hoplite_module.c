#include <ngx_config.h>
#include <ngx_core.h>
#include <ngx_http.h>

#include "hoplite_blob_host_provider.h"
#include "hoplite_hta.h"
#include "hoplite_host_provider.h"
#include "hoplite_runtime.h"
#include "hoplite_value_store_host_provider.h"

typedef struct {
    ngx_str_t bootstrap;
    ngx_str_t manifest;
} ngx_http_hoplite_main_conf_t;

typedef struct {
    ngx_str_t handler;
    uint64_t prepared_handler;
    ngx_uint_t app;
    ngx_uint_t adapter;
    ngx_flag_t request_body;
    size_t request_body_max;
    size_t request_body_chunk;
    size_t response_body_chunk;
} ngx_http_hoplite_loc_conf_t;

#define NGX_HTTP_HOPLITE_RAW 0
#define NGX_HTTP_HOPLITE_REQUEST 1
#define NGX_HTTP_HOPLITE_REQUEST_HTA 2

typedef struct {
    ngx_http_request_t *request;
    ngx_chain_t *chain;
    off_t offset;
    ngx_flag_t closed;
} ngx_http_hoplite_body_t;

typedef struct {
    uint64_t handle;
    uint64_t offset;
    uint64_t length;
} ngx_http_hoplite_source_plan_t;

typedef struct {
    uint64_t handle;
    uint64_t offset;
    uint64_t length;
    uint64_t remaining;
    size_t chunk_size;
    ngx_chain_t *free;
    ngx_chain_t *busy;
    ngx_chain_t *out;
    ngx_flag_t active;
    ngx_flag_t closed;
    ngx_flag_t final_read;
} ngx_http_hoplite_source_t;

typedef struct ngx_http_hoplite_ctx_s ngx_http_hoplite_ctx_t;
typedef struct ngx_http_hoplite_native_completion_s
    ngx_http_hoplite_native_completion_t;

typedef struct {
    ngx_event_t event;
    ngx_http_hoplite_ctx_t *ctx;
    uint64_t call;
} ngx_http_hoplite_sleep_t;


typedef struct ngx_http_hoplite_provider_s ngx_http_hoplite_provider_t;

typedef struct {
    ngx_http_hoplite_ctx_t *ctx;
    uint64_t work;
    uint64_t call;
    ngx_str_t operation;
    hoplite_hta_value_t *arguments;
    ngx_pool_t *pool;
    ngx_log_t *log;
} ngx_http_hoplite_host_call_t;

typedef ngx_int_t (*ngx_http_hoplite_provider_invoke_pt)(
    const ngx_http_hoplite_host_call_t *call);
typedef void (*ngx_http_hoplite_provider_cancel_pt)(
    ngx_http_hoplite_ctx_t *ctx);

struct ngx_http_hoplite_provider_s {
    hoplite_host_service_t service;
    ngx_http_hoplite_provider_invoke_pt invoke;
    ngx_http_hoplite_provider_cancel_pt cancel;
    uint32_t capabilities;
};

#define NGX_HTTP_HOPLITE_PROVIDER_REQUEST_BODY 0x01u
#define NGX_HTTP_HOPLITE_PROVIDER_RESPONSE_BODY 0x02u

struct ngx_http_hoplite_ctx_s {
    ngx_queue_t queue;
    ngx_http_request_t *request;
    uint64_t work;
    uint64_t response;
    ngx_flag_t queued;
    ngx_flag_t done;
    ngx_http_hoplite_sleep_t *sleep;
    const ngx_http_hoplite_provider_t *provider;
    const hoplite_host_provider_v1_t *native_provider;
    ngx_http_hoplite_native_completion_t *native_completion;
    ngx_http_hoplite_body_t body;
    ngx_http_hoplite_source_t source;
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
ngx_http_hoplite_header_at(void *data, size_t index,
                           hoplite_slice_t *name, hoplite_slice_t *value)
{
    ngx_http_request_t *request = data;
    ngx_list_part_t *part = &request->headers_in.headers.part;
    ngx_table_elt_t *headers = part->elts;
    size_t offset = 0;
    ngx_uint_t i;

    for (i = 0; ; i++) {
        if (i >= part->nelts) {
            if (part->next == NULL) {
                return 1;
            }
            part = part->next;
            headers = part->elts;
            i = 0;
        }
        if (offset++ == index) {
            name->data = headers[i].key.data;
            name->len = headers[i].key.len;
            value->data = headers[i].value.data;
            value->len = headers[i].value.len;
            return 0;
        }
    }
}

static size_t
ngx_http_hoplite_header_count(ngx_http_request_t *request)
{
    ngx_list_part_t *part = &request->headers_in.headers.part;
    size_t count = 0;
    while (part != NULL) {
        count += part->nelts;
        part = part->next;
    }
    return count;
}

static hoplite_slice_t
ngx_http_hoplite_slice(ngx_str_t value)
{
    hoplite_slice_t slice;
    slice.data = value.data;
    slice.len = value.len;
    return slice;
}

static hoplite_runtime_t *ngx_http_hoplite_runtime;
static ngx_queue_t ngx_http_hoplite_requests;
static ngx_flag_t ngx_http_hoplite_queue_ready;
static hoplite_host_registry_t ngx_http_hoplite_providers;

static ngx_int_t ngx_http_hoplite_handler(ngx_http_request_t *request);
static char *ngx_http_hoplite_content(ngx_conf_t *cf, ngx_command_t *cmd,
                                      void *conf);
static char *ngx_http_hoplite_app(ngx_conf_t *cf, ngx_command_t *cmd,
                                  void *conf);
static void *ngx_http_hoplite_create_main_conf(ngx_conf_t *cf);
static void *ngx_http_hoplite_create_loc_conf(ngx_conf_t *cf);
static char *ngx_http_hoplite_merge_loc_conf(ngx_conf_t *cf, void *parent,
                                             void *child);
static ngx_int_t ngx_http_hoplite_init_process(ngx_cycle_t *cycle);
static void ngx_http_hoplite_exit_process(ngx_cycle_t *cycle);
static void ngx_http_hoplite_cleanup(void *data);
static void ngx_http_hoplite_sleep_handler(ngx_event_t *event);
static ngx_int_t ngx_http_hoplite_drain(ngx_log_t *log);
static void ngx_http_hoplite_source_close(ngx_http_hoplite_ctx_t *ctx);
static void ngx_http_hoplite_source_write_handler(ngx_http_request_t *request);

static ngx_int_t ngx_http_hoplite_provider_register(
    const ngx_http_hoplite_provider_t *provider);
static const ngx_http_hoplite_provider_t *ngx_http_hoplite_provider_find(
    ngx_str_t service);
static ngx_int_t ngx_http_hoplite_nginx_invoke(
    const ngx_http_hoplite_host_call_t *call);
static void ngx_http_hoplite_nginx_cancel(ngx_http_hoplite_ctx_t *ctx);

static const ngx_http_hoplite_provider_t ngx_http_hoplite_nginx_provider = {
    {(const uint8_t *) "nginx", sizeof("nginx") - 1},
    ngx_http_hoplite_nginx_invoke,
    ngx_http_hoplite_nginx_cancel,
    0
};

static ngx_command_t ngx_http_hoplite_commands[] = {
    {
        ngx_string("hoplite_bootstrap"),
        NGX_HTTP_MAIN_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_str_slot,
        NGX_HTTP_MAIN_CONF_OFFSET,
        offsetof(ngx_http_hoplite_main_conf_t, bootstrap),
        NULL
    },
    {
        ngx_string("hoplite_manifest"),
        NGX_HTTP_MAIN_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_str_slot,
        NGX_HTTP_MAIN_CONF_OFFSET,
        offsetof(ngx_http_hoplite_main_conf_t, manifest),
        NULL
    },
    {
        ngx_string("hoplite_app"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1,
        ngx_http_hoplite_app,
        NGX_HTTP_LOC_CONF_OFFSET,
        0,
        NULL
    },
    {
        ngx_string("hoplite_content"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE12,
        ngx_http_hoplite_content,
        NGX_HTTP_LOC_CONF_OFFSET,
        0,
        NULL
    },
    {
        ngx_string("hoplite_request_body"),
        NGX_HTTP_LOC_CONF | NGX_CONF_FLAG,
        ngx_conf_set_flag_slot,
        NGX_HTTP_LOC_CONF_OFFSET,
        offsetof(ngx_http_hoplite_loc_conf_t, request_body),
        NULL
    },
    {
        ngx_string("hoplite_request_body_max"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_size_slot,
        NGX_HTTP_LOC_CONF_OFFSET,
        offsetof(ngx_http_hoplite_loc_conf_t, request_body_max),
        NULL
    },
    {
        ngx_string("hoplite_request_body_chunk"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_size_slot,
        NGX_HTTP_LOC_CONF_OFFSET,
        offsetof(ngx_http_hoplite_loc_conf_t, request_body_chunk),
        NULL
    },
    {
        ngx_string("hoplite_response_body_chunk"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_size_slot,
        NGX_HTTP_LOC_CONF_OFFSET,
        offsetof(ngx_http_hoplite_loc_conf_t, response_body_chunk),
        NULL
    },
    ngx_null_command
};

static ngx_http_module_t ngx_http_hoplite_module_ctx = {
    NULL,
    NULL,
    ngx_http_hoplite_create_main_conf,
    NULL,
    NULL,
    NULL,
    ngx_http_hoplite_create_loc_conf,
    ngx_http_hoplite_merge_loc_conf
};

ngx_module_t ngx_http_hoplite_module = {
    NGX_MODULE_V1,
    &ngx_http_hoplite_module_ctx,
    ngx_http_hoplite_commands,
    NGX_HTTP_MODULE,
    NULL,
    NULL,
    ngx_http_hoplite_init_process,
    NULL,
    NULL,
    ngx_http_hoplite_exit_process,
    NULL,
    NGX_MODULE_V1_PADDING
};

static ngx_int_t
ngx_http_hoplite_copy(ngx_pool_t *pool, const ngx_str_t *source,
                      ngx_str_t *destination)
{
    destination->len = source->len;
    if (source->len == 0) {
        destination->data = NULL;
        return NGX_OK;
    }
    destination->data = ngx_pnalloc(pool, source->len);
    if (destination->data == NULL) {
        return NGX_ERROR;
    }
    ngx_memcpy(destination->data, source->data, source->len);
    return NGX_OK;
}

static ngx_http_hoplite_ctx_t *
ngx_http_hoplite_find(uint64_t work)
{
    ngx_queue_t *cursor;
    ngx_http_hoplite_ctx_t *ctx;

    if (!ngx_http_hoplite_queue_ready) {
        return NULL;
    }
    for (cursor = ngx_queue_head(&ngx_http_hoplite_requests);
         cursor != ngx_queue_sentinel(&ngx_http_hoplite_requests);
         cursor = ngx_queue_next(cursor))
    {
        ctx = ngx_queue_data(cursor, ngx_http_hoplite_ctx_t, queue);
        if (ctx->work == work) {
            return ctx;
        }
    }
    return NULL;
}

static void
ngx_http_hoplite_source_close(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_source_t *source;

    if (ctx == NULL) {
        return;
    }
    source = &ctx->source;
    if (!source->active || source->closed || source->handle == 0) {
        return;
    }
    source->closed = 1;
    if (hoplite_blob_host_provider_response_close_v1(
            ctx,
            ctx->work,
            source->handle) != HOPLITE_BLOB_HOST_PROVIDER_OK)
    {
        ngx_log_error(NGX_LOG_ERR, ctx->request->connection->log, 0,
                      "hoplite could not close response source");
    }
}

static void
ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_source_close(ctx);
    if (ctx->done) {
        return;
    }
    ctx->done = 1;
    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {
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
        ngx_queue_remove(&ctx->queue);
        ctx->queued = 0;
    }
    if (ctx->work != 0) {
        (void) hoplite_blob_host_provider_release_work_v1(ctx->work);
    }
    if (ngx_http_hoplite_runtime != NULL && ctx->work != 0) {
        (void) hoplite_work_close(ngx_http_hoplite_runtime, ctx->work);
    }
}

static ngx_int_t
ngx_http_hoplite_add_headers(ngx_http_request_t *request,
                             const hoplite_hta_value_t *headers)
{
    size_t i;
    ngx_str_t key, value, copied_key, copied_value;
    ngx_table_elt_t *header;
    hoplite_hta_pair_t *pair;

    if (headers == NULL
        || (headers->kind != HOPLITE_HTA_MAP
            && headers->kind != HOPLITE_HTA_OBJECT))
    {
        return NGX_OK;
    }

    for (i = 0; i < headers->as.map.count; i++) {
        pair = &headers->as.map.items[i];
        if (hoplite_hta_text(pair->key, &key) != NGX_OK
            || hoplite_hta_text(pair->value, &value) != NGX_OK)
        {
            continue;
        }

        if (key.len == sizeof("content-type") - 1
            && ngx_strncasecmp(key.data, (u_char *) "content-type", key.len) == 0)
        {
            if (ngx_http_hoplite_copy(request->pool, &value, &copied_value)
                != NGX_OK)
            {
                return NGX_ERROR;
            }
            request->headers_out.content_type = copied_value;
            continue;
        }

        if (key.len == sizeof("content-length") - 1
            && ngx_strncasecmp(key.data, (u_char *) "content-length", key.len) == 0)
        {
            continue;
        }

        header = ngx_list_push(&request->headers_out.headers);
        if (header == NULL
            || ngx_http_hoplite_copy(request->pool, &key, &copied_key) != NGX_OK
            || ngx_http_hoplite_copy(request->pool, &value, &copied_value) != NGX_OK)
        {
            return NGX_ERROR;
        }
        header->hash = 1;
        header->key = copied_key;
        header->value = copied_value;
    }
    return NGX_OK;
}

static void
ngx_http_hoplite_send(ngx_http_hoplite_ctx_t *ctx,
                      ngx_uint_t status,
                      const ngx_str_t *body,
                      const hoplite_hta_value_t *headers)
{
    ngx_http_request_t *request = ctx->request;
    ngx_buf_t *buffer;
    ngx_chain_t chain;
    ngx_int_t rc;
    ngx_str_t copied_body;

    if (ctx->done) {
        return;
    }
    if (ngx_http_hoplite_copy(request->pool, body, &copied_body) != NGX_OK
        || ngx_http_hoplite_add_headers(request, headers) != NGX_OK)
    {
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, NGX_HTTP_INTERNAL_SERVER_ERROR);
        return;
    }

    request->headers_out.status = status;
    request->headers_out.content_length_n = (off_t) copied_body.len;
    if (request->headers_out.content_type.len == 0) {
        ngx_str_set(&request->headers_out.content_type, "text/plain");
    }

    rc = ngx_http_send_header(request);
    if (rc == NGX_ERROR || rc > NGX_OK || request->header_only
        || copied_body.len == 0)
    {
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, rc);
        return;
    }

    buffer = ngx_calloc_buf(request->pool);
    if (buffer == NULL) {
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, NGX_HTTP_INTERNAL_SERVER_ERROR);
        return;
    }

    buffer->pos = copied_body.data;
    buffer->last = copied_body.data + copied_body.len;
    buffer->memory = 1;
    buffer->last_buf = 1;
    chain.buf = buffer;
    chain.next = NULL;

    rc = ngx_http_output_filter(request, &chain);
    ngx_http_hoplite_finish(ctx);
    ngx_http_finalize_request(request, rc);
}

static ngx_flag_t
ngx_http_hoplite_text_equal(const hoplite_hta_value_t *value,
                            const char *literal)
{
    size_t length;

    if (value == NULL || value->kind != HOPLITE_HTA_STRING) {
        return 0;
    }
    length = ngx_strlen(literal);
    return value->as.text.len == length
        && ngx_memcmp(value->as.text.data, literal, length) == 0;
}

static ngx_int_t
ngx_http_hoplite_source_plan(const hoplite_hta_value_t *body,
                             ngx_http_hoplite_source_plan_t *plan)
{
    hoplite_hta_value_t *protocol, *source, *offset, *length;
    int64_t source_number, offset_number, length_number;

    if (body == NULL
        || (body->kind != HOPLITE_HTA_MAP
            && body->kind != HOPLITE_HTA_OBJECT))
    {
        return NGX_DECLINED;
    }
    protocol = hoplite_hta_map_get(body, "protocol");
    if (protocol == NULL) {
        return NGX_DECLINED;
    }
    source = hoplite_hta_map_get(body, "source");
    offset = hoplite_hta_map_get(body, "offset");
    length = hoplite_hta_map_get(body, "length");
    if (body->as.map.count != 4
        || !ngx_http_hoplite_text_equal(
               protocol, "hoplite.response-source/1")
        || hoplite_hta_number(source, &source_number) != NGX_OK
        || hoplite_hta_number(offset, &offset_number) != NGX_OK
        || hoplite_hta_number(length, &length_number) != NGX_OK
        || source_number <= 0
        || offset_number < 0
        || length_number < 0
        || offset_number > INT64_MAX - length_number)
    {
        return NGX_ERROR;
    }
    plan->handle = (uint64_t) source_number;
    plan->offset = (uint64_t) offset_number;
    plan->length = (uint64_t) length_number;
    return NGX_OK;
}

static ngx_flag_t
ngx_http_hoplite_source_pending(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_request_t *request = ctx->request;
    ngx_connection_t *connection = request->connection;

    return ctx->source.busy != NULL
        || request->buffered
        || request->postponed != NULL
        || (request == request->main && connection->buffered);
}

static ngx_int_t
ngx_http_hoplite_source_wait(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_request_t *request = ctx->request;
    ngx_connection_t *connection = request->connection;
    ngx_event_t *write = connection->write;
    ngx_http_core_loc_conf_t *core;

    core = ngx_http_get_module_loc_conf(request->main, ngx_http_core_module);
    request->write_event_handler = ngx_http_hoplite_source_write_handler;
    if (!write->delayed && !write->timer_set) {
        ngx_add_timer(write, core->send_timeout);
    }
    if (ngx_handle_write_event(write, core->send_lowat) != NGX_OK) {
        return NGX_ERROR;
    }
    return NGX_AGAIN;
}

static ngx_chain_t *
ngx_http_hoplite_source_buffer(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_source_t *source = &ctx->source;
    ngx_chain_t *chain;
    ngx_buf_t *buffer;

    chain = ngx_chain_get_free_buf(ctx->request->pool, &source->free);
    if (chain == NULL) {
        return NULL;
    }
    buffer = chain->buf;
    if (buffer->start == NULL) {
        buffer->start = ngx_palloc(ctx->request->pool, source->chunk_size);
        if (buffer->start == NULL) {
            return NULL;
        }
        buffer->end = buffer->start + source->chunk_size;
    }
    buffer->pos = buffer->start;
    buffer->last = buffer->start;
    buffer->tag = (ngx_buf_tag_t) &ngx_http_hoplite_module;
    buffer->temporary = 1;
    buffer->memory = 0;
    buffer->in_file = 0;
    buffer->flush = 0;
    buffer->sync = 0;
    buffer->last_buf = 0;
    buffer->last_in_chain = 0;
    chain->next = NULL;
    return chain;
}

static ngx_int_t
ngx_http_hoplite_source_read(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_source_t *source = &ctx->source;
    ngx_chain_t *chain;
    ngx_buf_t *buffer;
    size_t capacity, returned = 0;

    if (source->remaining == 0) {
        return NGX_DONE;
    }
    chain = ngx_http_hoplite_source_buffer(ctx);
    if (chain == NULL) {
        return NGX_ERROR;
    }
    capacity = source->remaining < (uint64_t) source->chunk_size
        ? (size_t) source->remaining
        : source->chunk_size;
    buffer = chain->buf;
    if (hoplite_blob_host_provider_response_read_v1(
            ctx,
            ctx->work,
            source->handle,
            buffer->start,
            capacity,
            &returned) != HOPLITE_BLOB_HOST_PROVIDER_OK
        || returned == 0
        || returned > capacity)
    {
        return NGX_ERROR;
    }
    buffer->pos = buffer->start;
    buffer->last = buffer->start + returned;
    source->remaining -= returned;
    source->final_read = source->remaining == 0;
    if (source->final_read) {
        buffer->last_buf = ctx->request == ctx->request->main;
        buffer->last_in_chain = 1;
        ngx_http_hoplite_source_close(ctx);
    } else {
        buffer->flush = 1;
    }
    source->out = chain;
    return NGX_OK;
}

static void
ngx_http_hoplite_source_complete(ngx_http_hoplite_ctx_t *ctx, ngx_int_t rc)
{
    ngx_event_t *write = ctx->request->connection->write;

    if (write->timer_set) {
        ngx_del_timer(write);
    }
    ctx->source.active = 0;
    ngx_http_hoplite_finish(ctx);
    ngx_http_finalize_request(ctx->request, rc);
}

static void
ngx_http_hoplite_source_fail(ngx_http_hoplite_ctx_t *ctx, ngx_int_t rc,
                             const char *message)
{
    ngx_event_t *write = ctx->request->connection->write;

    ngx_log_error(NGX_LOG_ERR, ctx->request->connection->log, 0,
                  "%s", message);
    if (write->timer_set) {
        ngx_del_timer(write);
    }
    ngx_http_hoplite_source_close(ctx);
    ctx->source.active = 0;
    ngx_http_hoplite_finish(ctx);
    ngx_http_finalize_request(ctx->request, rc);
}

static void
ngx_http_hoplite_source_pump(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_request_t *request = ctx->request;
    ngx_http_hoplite_source_t *source = &ctx->source;
    ngx_int_t rc;

    while (source->active && !ctx->done) {
        if (source->out == NULL) {
            if (ngx_http_hoplite_source_read(ctx) != NGX_OK) {
                ngx_http_hoplite_source_fail(
                    ctx, NGX_ERROR,
                    "hoplite response source ended before its declared length");
                return;
            }
        }
        rc = ngx_http_output_filter(request, source->out);
        ngx_chain_update_chains(
            request->pool,
            &source->free,
            &source->busy,
            &source->out,
            (ngx_buf_tag_t) &ngx_http_hoplite_module);
        if (rc == NGX_ERROR) {
            ngx_http_hoplite_source_fail(
                ctx, NGX_ERROR,
                "hoplite response source output failed");
            return;
        }
        if (rc == NGX_AGAIN || ngx_http_hoplite_source_pending(ctx)) {
            if (ngx_http_hoplite_source_wait(ctx) == NGX_ERROR) {
                ngx_http_hoplite_source_fail(
                    ctx, NGX_ERROR,
                    "hoplite response source could not wait for output");
            }
            return;
        }
        if (source->final_read) {
            ngx_http_hoplite_source_complete(ctx, rc);
            return;
        }
    }
}

static void
ngx_http_hoplite_source_write_handler(ngx_http_request_t *request)
{
    ngx_http_hoplite_ctx_t *ctx;
    ngx_http_hoplite_source_t *source;
    ngx_connection_t *connection = request->connection;
    ngx_event_t *write = connection->write;
    ngx_int_t rc;

    ctx = ngx_http_get_module_ctx(request, ngx_http_hoplite_module);
    if (ctx == NULL || ctx->done || !ctx->source.active) {
        return;
    }
    source = &ctx->source;
    if (write->timedout) {
        connection->timedout = 1;
        ngx_http_hoplite_source_fail(
            ctx, NGX_HTTP_REQUEST_TIME_OUT,
            "client timed out while receiving a Hoplite response source");
        return;
    }
    if (write->delayed) {
        if (ngx_http_hoplite_source_wait(ctx) == NGX_ERROR) {
            ngx_http_hoplite_source_fail(
                ctx, NGX_ERROR,
                "hoplite response source could not resume delayed output");
        }
        return;
    }
    rc = ngx_http_output_filter(request, NULL);
    ngx_chain_update_chains(
        request->pool,
        &source->free,
        &source->busy,
        &source->out,
        (ngx_buf_tag_t) &ngx_http_hoplite_module);
    if (rc == NGX_ERROR) {
        ngx_http_hoplite_source_fail(
            ctx, NGX_ERROR,
            "hoplite response source flush failed");
        return;
    }
    if (rc == NGX_AGAIN || ngx_http_hoplite_source_pending(ctx)) {
        if (ngx_http_hoplite_source_wait(ctx) == NGX_ERROR) {
            ngx_http_hoplite_source_fail(
                ctx, NGX_ERROR,
                "hoplite response source could not wait after flushing");
        }
        return;
    }
    if (write->timer_set) {
        ngx_del_timer(write);
    }
    if (source->final_read) {
        ngx_http_hoplite_source_complete(ctx, rc);
        return;
    }
    ngx_http_hoplite_source_pump(ctx);
}

static void
ngx_http_hoplite_send_source(
    ngx_http_hoplite_ctx_t *ctx,
    ngx_uint_t status,
    const hoplite_hta_value_t *headers,
    const ngx_http_hoplite_source_plan_t *plan)
{
    ngx_http_request_t *request = ctx->request;
    ngx_http_hoplite_loc_conf_t *conf;
    ngx_str_t error = ngx_string("Hoplite response source is unavailable\n");
    ngx_int_t rc;

    if (ctx->done || ctx->source.active || plan->length > (uint64_t) NGX_MAX_OFF_T_VALUE) {
        ngx_http_hoplite_send(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &error, NULL);
        return;
    }
    conf = ngx_http_get_module_loc_conf(request, ngx_http_hoplite_module);
    if (conf == NULL || conf->response_body_chunk == 0) {
        ngx_http_hoplite_send(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &error, NULL);
        return;
    }
    ctx->source.handle = plan->handle;
    ctx->source.offset = plan->offset;
    ctx->source.length = plan->length;
    ctx->source.remaining = plan->length;
    ctx->source.chunk_size = conf->response_body_chunk;
    ctx->source.active = 1;

    if (!request->header_only && plan->length != 0
        && ngx_http_hoplite_source_read(ctx) != NGX_OK)
    {
        ngx_http_hoplite_source_close(ctx);
        ctx->source.active = 0;
        ngx_http_hoplite_send(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &error, NULL);
        return;
    }
    if (ngx_http_hoplite_add_headers(request, headers) != NGX_OK) {
        ngx_http_hoplite_source_close(ctx);
        ctx->source.active = 0;
        ngx_http_hoplite_send(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &error, NULL);
        return;
    }
    request->headers_out.status = status;
    request->headers_out.content_length_n = (off_t) plan->length;
    request->single_range = 1;
    request->allow_ranges = 0;
    if (request->headers_out.content_type.len == 0) {
        ngx_str_set(&request->headers_out.content_type,
                    "application/octet-stream");
    }
    rc = ngx_http_send_header(request);
    if (rc == NGX_ERROR || rc > NGX_OK
        || request->header_only || plan->length == 0)
    {
        ngx_http_hoplite_source_close(ctx);
        ctx->source.active = 0;
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, rc);
        return;
    }
    ngx_http_hoplite_source_pump(ctx);
}

static ngx_int_t
ngx_http_hoplite_send_native(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_request_t *request = ctx->request;
    hoplite_slice_t body, key, value;
    ngx_table_elt_t *header;
    ngx_buf_t *buffer;
    ngx_chain_t chain;
    ngx_int_t rc;
    uint16_t status;
    size_t count, i;

    if (hoplite_response_status_v2(ngx_http_hoplite_runtime, ctx->response,
                                   &status) != 0
        || hoplite_response_body_v2(ngx_http_hoplite_runtime, ctx->response,
                                    &body) != 0)
    {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    count = hoplite_response_header_count_v2(ngx_http_hoplite_runtime,
                                              ctx->response);
    for (i = 0; i < count; i++) {
        if (hoplite_response_header_at_v2(ngx_http_hoplite_runtime,
                                          ctx->response, i,
                                          &key, &value) != 0)
        {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (key.len == sizeof("content-length") - 1
            && ngx_strncasecmp((u_char *) key.data,
                               (u_char *) "content-length", key.len) == 0)
        {
            continue;
        }
        if (key.len == sizeof("content-type") - 1
            && ngx_strncasecmp((u_char *) key.data,
                               (u_char *) "content-type", key.len) == 0)
        {
            request->headers_out.content_type.data = (u_char *) value.data;
            request->headers_out.content_type.len = value.len;
            continue;
        }
        header = ngx_list_push(&request->headers_out.headers);
        if (header == NULL) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        header->hash = 1;
        header->key.data = (u_char *) key.data;
        header->key.len = key.len;
        header->value.data = (u_char *) value.data;
        header->value.len = value.len;
    }

    request->headers_out.status = status;
    request->headers_out.content_length_n = (off_t) body.len;
    if (request->headers_out.content_type.len == 0) {
        ngx_str_set(&request->headers_out.content_type, "text/plain");
    }
    rc = ngx_http_send_header(request);
    if (rc == NGX_ERROR || rc > NGX_OK || request->header_only || body.len == 0) {
        ctx->done = 1;
        return rc;
    }

    buffer = ngx_calloc_buf(request->pool);
    if (buffer == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    buffer->pos = (u_char *) body.data;
    buffer->last = (u_char *) body.data + body.len;
    buffer->memory = 1;
    buffer->last_buf = 1;
    chain.buf = buffer;
    chain.next = NULL;
    rc = ngx_http_output_filter(request, &chain);
    ctx->done = 1;
    return rc;
}

static void
ngx_http_hoplite_send_result(ngx_http_hoplite_ctx_t *ctx,
                             const hoplite_hta_value_t *payload)
{
    hoplite_hta_value_t *status_value;
    hoplite_hta_value_t *body_value;
    hoplite_hta_value_t *headers;
    ngx_http_hoplite_source_plan_t source_plan;
    ngx_int_t source_status;
    int64_t status_number = NGX_HTTP_OK;
    ngx_str_t body = ngx_null_string;

    if (payload == NULL
        || (payload->kind != HOPLITE_HTA_MAP
            && payload->kind != HOPLITE_HTA_OBJECT))
    {
        ngx_str_set(&body, "Hoplite handler must return a response map\n");
        ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
        return;
    }

    status_value = hoplite_hta_map_get(payload, "status");
    body_value = hoplite_hta_map_get(payload, "body");
    headers = hoplite_hta_map_get(payload, "headers");

    if (status_value != NULL
        && hoplite_hta_number(status_value, &status_number) != NGX_OK)
    {
        status_number = NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    if (status_number < 100 || status_number > 599) {
        status_number = NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    source_status = ngx_http_hoplite_source_plan(body_value, &source_plan);
    if (source_status == NGX_ERROR
        || (source_status == NGX_OK
            && status_number != NGX_HTTP_OK
            && status_number != NGX_HTTP_PARTIAL_CONTENT))
    {
        ngx_str_set(&body, "Hoplite response source plan is invalid\n");
        ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
        return;
    }
    if (source_status == NGX_OK) {
        ngx_http_hoplite_send_source(
            ctx, (ngx_uint_t) status_number, headers, &source_plan);
        return;
    }

    if (body_value != NULL && body_value->kind != HOPLITE_HTA_NIL
        && hoplite_hta_text(body_value, &body) != NGX_OK)
    {
        ngx_str_set(&body, "Hoplite response body must be a string or bytes\n");
        status_number = NGX_HTTP_INTERNAL_SERVER_ERROR;
        headers = NULL;
    }

    ngx_http_hoplite_send(ctx, (ngx_uint_t) status_number, &body, headers);
}

static void
ngx_http_hoplite_send_error(ngx_http_hoplite_ctx_t *ctx,
                            const hoplite_hta_value_t *payload)
{
    hoplite_hta_value_t *message_value;
    ngx_str_t message;

    if (payload != NULL
        && (payload->kind == HOPLITE_HTA_MAP
            || payload->kind == HOPLITE_HTA_OBJECT))
    {
        message_value = hoplite_hta_map_get(payload, "message");
        if (message_value != NULL
            && hoplite_hta_text(message_value, &message) == NGX_OK)
        {
            ngx_log_error(NGX_LOG_ERR, ctx->request->connection->log, 0,
                          "hoplite handler error: %V", &message);
        }
    }

    ngx_str_set(&message, "Hoplite handler failed\n");
    ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &message, NULL);
}

static ngx_int_t
ngx_http_hoplite_reject(uint64_t call, ngx_pool_t *pool, const char *message)
{
    ngx_str_t text, encoded;
    text.data = (u_char *) message;
    text.len = ngx_strlen(message);
    if (hoplite_hta_encode_string(pool, &text, &encoded) != NGX_OK) {
        return NGX_ERROR;
    }
    return hoplite_call_reject(ngx_http_hoplite_runtime, call,
                               encoded.data, encoded.len) == 0
        ? NGX_OK : NGX_ERROR;
}

static ngx_int_t
ngx_http_hoplite_provider_register(
    const ngx_http_hoplite_provider_t *provider)
{
    if (provider == NULL || provider->invoke == NULL) {
        return NGX_ERROR;
    }
    return hoplite_host_registry_register(&ngx_http_hoplite_providers,
                                          provider->service,
                                          provider)
               == HOPLITE_HOST_REGISTRY_OK
        ? NGX_OK : NGX_ERROR;
}

static const ngx_http_hoplite_provider_t *
ngx_http_hoplite_provider_find(ngx_str_t service)
{
    hoplite_host_service_t lookup;
    lookup.data = service.data;
    lookup.len = service.len;
    return hoplite_host_registry_find(&ngx_http_hoplite_providers, lookup);
}

static ngx_flag_t
ngx_http_hoplite_native_request_body_allowed(
    ngx_http_hoplite_ctx_t *ctx,
    uint64_t work)
{
    return ctx != NULL
        && !ctx->done
        && ctx->work == work
        && ngx_http_hoplite_runtime != NULL
        && ctx->native_provider != NULL
        && (ctx->native_provider->capabilities
            & HOPLITE_HOST_PROVIDER_REQUEST_BODY) != 0;
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
    ngx_http_hoplite_ctx_t *ctx = request_context;

    if (returned != NULL) {
        *returned = 0;
    }
    if (!ngx_http_hoplite_native_request_body_allowed(ctx, work)
        || handle == 0 || output == NULL || capacity == 0 || returned == NULL)
    {
        return HOPLITE_HOST_RESOURCE_ERROR;
    }
    return hoplite_request_body_read_v3(
               ngx_http_hoplite_runtime,
               work,
               handle,
               output,
               capacity,
               returned) == 0
        ? HOPLITE_HOST_RESOURCE_OK
        : HOPLITE_HOST_RESOURCE_ERROR;
}

int32_t
hoplite_host_request_body_finish_v1(
    void *request_context,
    uint64_t work,
    uint64_t handle)
{
    ngx_http_hoplite_ctx_t *ctx = request_context;

    if (!ngx_http_hoplite_native_request_body_allowed(ctx, work)
        || handle == 0)
    {
        return HOPLITE_HOST_RESOURCE_ERROR;
    }
    return hoplite_request_body_finish_v3(
               ngx_http_hoplite_runtime,
               work,
               handle) == 0
        ? HOPLITE_HOST_RESOURCE_OK
        : HOPLITE_HOST_RESOURCE_ERROR;
}

static int32_t
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
            ngx_str_t body = ngx_string("Hoplite runtime delivery failed\n");
            ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR,
                                  &body, NULL);
        }
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (completion->retained
        && ngx_http_hoplite_drain(completion->log) != NGX_OK)
    {
        ngx_str_t body = ngx_string("Hoplite runtime delivery failed\n");
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

static void
ngx_http_hoplite_nginx_cancel(ngx_http_hoplite_ctx_t *ctx)
{
    if (ctx == NULL || ctx->sleep == NULL) {
        return;
    }
    if (ctx->sleep->event.timer_set) {
        ngx_del_timer(&ctx->sleep->event);
    }
    ctx->sleep = NULL;
}

static ngx_int_t
ngx_http_hoplite_nginx_invoke(const ngx_http_hoplite_host_call_t *call)
{
    int64_t delay;
    ngx_http_hoplite_sleep_t *sleep;

    if (call->operation.len != sizeof("sleep") - 1
        || ngx_strncmp(call->operation.data, "sleep", call->operation.len) != 0)
    {
        return ngx_http_hoplite_reject(call->call, call->pool,
                                       "unsupported nginx host operation");
    }
    if (call->arguments == NULL
        || call->arguments->kind != HOPLITE_HTA_VECTOR
        || call->arguments->as.vector.count != 1
        || hoplite_hta_number(call->arguments->as.vector.items[0], &delay)
               != NGX_OK
        || delay < 0 || delay > 3600000)
    {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "nginx/sleep expects milliseconds from 0 to 3600000");
    }
    if (delay == 0) {
        return hoplite_call_resolve(ngx_http_hoplite_runtime,
                                    call->call, NULL, 0) == 0
            ? NGX_OK : NGX_ERROR;
    }
    if (call->ctx->sleep != NULL) {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "request already has a pending Hoplite operation");
    }

    sleep = ngx_pcalloc(call->ctx->request->pool, sizeof(*sleep));
    if (sleep == NULL) {
        return NGX_ERROR;
    }
    sleep->ctx = call->ctx;
    sleep->call = call->call;
    sleep->event.handler = ngx_http_hoplite_sleep_handler;
    sleep->event.data = sleep;
    sleep->event.log = call->log;
    call->ctx->sleep = sleep;
    ngx_add_timer(&sleep->event, (ngx_msec_t) delay);
    return NGX_AGAIN;
}

static ngx_int_t
ngx_http_hoplite_host_call(hoplite_hta_value_t *event,
                           ngx_pool_t *pool,
                           ngx_log_t *log)
{
    int64_t call_number, work_number;
    ngx_str_t service, operation;
    ngx_http_hoplite_ctx_t *ctx;
    const ngx_http_hoplite_provider_t *provider;
    const hoplite_host_provider_v1_t *native_provider;
    hoplite_host_service_t lookup;
    ngx_http_hoplite_host_call_t call;
    hoplite_host_call_v1_t native_call;
    ngx_http_hoplite_native_completion_t *completion;
    ngx_str_t operation_copy, arguments_hta;
    ngx_int_t rc;
    int32_t native_rc;

    if (event->as.vector.count != 8
        || hoplite_hta_number(event->as.vector.items[1], &call_number) != NGX_OK
        || hoplite_hta_number(event->as.vector.items[2], &work_number) != NGX_OK
        || call_number <= 0 || work_number <= 0
        || hoplite_hta_text(event->as.vector.items[5], &service) != NGX_OK
        || hoplite_hta_text(event->as.vector.items[6], &operation) != NGX_OK)
    {
        return NGX_ERROR;
    }

    ctx = ngx_http_hoplite_find((uint64_t) work_number);
    if (ctx == NULL || ctx->done) {
        return NGX_DECLINED;
    }
    if (ctx->provider != NULL || ctx->native_provider != NULL
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

    ctx->native_provider = native_provider;
    native_rc = native_provider->invoke(&native_call);
    if (completion->completed) {
        return completion->failed ? NGX_ERROR : NGX_OK;
    }
    if (native_rc == HOPLITE_HOST_PROVIDER_PENDING) {
        completion->retained = 1;
        return NGX_OK;
    }

    completion->completed = 1;
    ctx->native_completion = NULL;
    ctx->native_provider = NULL;
    if (native_rc == HOPLITE_HOST_PROVIDER_OK) {
        return ngx_http_hoplite_reject(
            (uint64_t) call_number, pool,
            "native provider returned without completing");
    }
    return NGX_ERROR;
}

static ngx_int_t
ngx_http_hoplite_dispatch(hoplite_hta_value_t *event,
                          ngx_pool_t *pool,
                          ngx_log_t *log)
{
    int64_t kind, work;
    ngx_http_hoplite_ctx_t *ctx;

    if (event == NULL || event->kind != HOPLITE_HTA_VECTOR
        || event->as.vector.count < 3
        || hoplite_hta_number(event->as.vector.items[0], &kind) != NGX_OK)
    {
        return NGX_ERROR;
    }

    if (kind == 2) {
        return ngx_http_hoplite_host_call(event, pool, log);
    }

    if (event->as.vector.count != 3
        || hoplite_hta_number(event->as.vector.items[1], &work) != NGX_OK)
    {
        return NGX_ERROR;
    }
    ctx = ngx_http_hoplite_find((uint64_t) work);
    if (ctx == NULL || ctx->done) {
        return NGX_DECLINED;
    }

    if (kind == 0) {
        ngx_http_hoplite_send_result(ctx, event->as.vector.items[2]);
        return NGX_OK;
    }
    if (kind == 1) {
        ngx_http_hoplite_send_error(ctx, event->as.vector.items[2]);
        return NGX_OK;
    }
    return NGX_ERROR;
}

static ngx_int_t
ngx_http_hoplite_drain(ngx_log_t *log)
{
    hoplite_buffer_t buffer;
    hoplite_hta_value_t *event;
    ngx_pool_t *pool;
    ngx_int_t rc = NGX_OK;

    if (ngx_http_hoplite_runtime == NULL) {
        return NGX_ERROR;
    }

    while (hoplite_work_poll(ngx_http_hoplite_runtime) != 0) {
        buffer.data = NULL;
        buffer.len = 0;
        if (hoplite_work_next_event(ngx_http_hoplite_runtime, &buffer) != 0) {
            return NGX_ERROR;
        }

        pool = ngx_create_pool(4096, log);
        if (pool == NULL) {
            hoplite_buffer_free(buffer.data, buffer.len);
            return NGX_ERROR;
        }
        if (hoplite_hta_decode(pool, buffer.data, buffer.len, &event) != NGX_OK
            || ngx_http_hoplite_dispatch(event, pool, log) == NGX_ERROR)
        {
            ngx_log_error(NGX_LOG_ERR, log, 0,
                          "hoplite received an invalid runtime event");
            rc = NGX_ERROR;
        }
        ngx_destroy_pool(pool);
        hoplite_buffer_free(buffer.data, buffer.len);
    }
    return rc;
}

static void
ngx_http_hoplite_sleep_handler(ngx_event_t *event)
{
    ngx_http_hoplite_sleep_t *sleep = event->data;
    ngx_http_hoplite_ctx_t *ctx = sleep->ctx;

    if (ctx == NULL || ctx->done || ctx->sleep != sleep) {
        return;
    }
    ctx->sleep = NULL;
    ctx->provider = NULL;
    if (hoplite_call_resolve(ngx_http_hoplite_runtime, sleep->call, NULL, 0) != 0
        || ngx_http_hoplite_drain(event->log) != NGX_OK)
    {
        ngx_str_t body = ngx_string("Hoplite runtime delivery failed\n");
        ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
    }
}

static void
ngx_http_hoplite_cleanup(void *data)
{
    ngx_http_hoplite_ctx_t *ctx = data;

    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {
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
    ngx_http_hoplite_source_close(ctx);

    if (ctx->work != 0) {
        (void) hoplite_blob_host_provider_release_work_v1(ctx->work);
    }
    if (!ctx->done && ngx_http_hoplite_runtime != NULL && ctx->work != 0) {
        (void) hoplite_work_cancel(ngx_http_hoplite_runtime, ctx->work);
        (void) hoplite_work_close(ngx_http_hoplite_runtime, ctx->work);
    }
    if (ngx_http_hoplite_runtime != NULL && ctx->response != 0) {
        (void) hoplite_response_close_v2(ngx_http_hoplite_runtime,
                                         ctx->response);
        ctx->response = 0;
    }
    if (ctx->queued) {
        ngx_queue_remove(&ctx->queue);
        ctx->queued = 0;
    }
    ctx->done = 1;
}

static int32_t
ngx_http_hoplite_body_read(void *data, uint8_t *output,
                           size_t capacity, size_t *returned)
{
    ngx_http_hoplite_body_t *body = data;
    ngx_buf_t *buffer;
    off_t length, available;
    ssize_t read;
    size_t total = 0, amount;

    if (body == NULL || output == NULL || returned == NULL || body->closed) {
        return 1;
    }

    while (total < capacity && body->chain != NULL) {
        buffer = body->chain->buf;
        length = ngx_buf_size(buffer);
        if (length < 0 || body->offset < 0 || body->offset > length) {
            return 1;
        }
        if (body->offset == length) {
            body->chain = body->chain->next;
            body->offset = 0;
            continue;
        }
        available = length - body->offset;
        amount = capacity - total;
        if (available < (off_t) amount) {
            amount = (size_t) available;
        }

        if (ngx_buf_in_memory(buffer)) {
            ngx_memcpy(output + total,
                       buffer->pos + (size_t) body->offset,
                       amount);
        } else if (buffer->in_file && buffer->file != NULL) {
            read = ngx_read_file(buffer->file,
                                 output + total,
                                 amount,
                                 buffer->file_pos + body->offset);
            if (read < 0 || (size_t) read != amount) {
                return 1;
            }
        } else {
            return 1;
        }

        total += amount;
        body->offset += (off_t) amount;
    }

    *returned = total;
    return HOPLITE_CALLBACK_OK;
}

static void
ngx_http_hoplite_body_close(void *data)
{
    ngx_http_hoplite_body_t *body = data;
    if (body == NULL || body->closed) {
        return;
    }
    body->closed = 1;
    body->request = NULL;
    body->chain = NULL;
    body->offset = 0;
}

static ngx_int_t
ngx_http_hoplite_invoke(ngx_http_request_t *request,
                        ngx_http_hoplite_ctx_t *ctx,
                        ngx_http_hoplite_loc_conf_t *conf,
                        const hoplite_request_body_v1 *body)
{
    hoplite_request_v2_t native_request;
    hoplite_request_v3_t native_request_v3;
    hoplite_outcome_v2_t outcome;
    ngx_str_t binding;
    ngx_int_t rc;

    native_request.context = request;
    native_request.method = ngx_http_hoplite_slice(request->method_name);
    native_request.uri = ngx_http_hoplite_slice(request->unparsed_uri);
    native_request.path = ngx_http_hoplite_slice(request->uri);
    native_request.query_string = ngx_http_hoplite_slice(request->args);
    native_request.remote_address =
        ngx_http_hoplite_slice(request->connection->addr_text);
    native_request.header_count = ngx_http_hoplite_header_count(request);
    native_request.header_at = ngx_http_hoplite_header_at;

    if (conf->app != NGX_CONF_UNSET_UINT) {
        if (body == NULL) {
            rc = hoplite_app_invoke_v2(ngx_http_hoplite_runtime, conf->app,
                                       &native_request, &outcome);
        } else {
            native_request_v3.request = native_request;
            native_request_v3.body = body;
            native_request_v3.max_body_bytes = conf->request_body_max;
            native_request_v3.max_chunk_bytes = conf->request_body_chunk;
            native_request_v3.require_declared_length = 1;
            rc = hoplite_app_invoke_v3(ngx_http_hoplite_runtime, conf->app,
                                       &native_request_v3, &outcome);
        }
        if (rc != 0) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (outcome.kind == 1) {
            /* send_native borrows response slices until request cleanup. */
            ctx->response = outcome.id;
            return ngx_http_hoplite_send_native(ctx);
        }
        if (outcome.kind != 2 || outcome.id == 0) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        ctx->work = outcome.id;
    } else if (conf->adapter == NGX_HTTP_HOPLITE_REQUEST_HTA) {
        if (body != NULL) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (hoplite_hta_encode_request(request, &binding) != NGX_OK) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        ctx->work = hoplite_work_call(ngx_http_hoplite_runtime,
                                      conf->prepared_handler,
                                      binding.data, binding.len);
        if (ctx->work == 0) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
    } else {
        if (conf->prepared_handler == 0) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (body == NULL) {
            rc = hoplite_handler_invoke_v2(ngx_http_hoplite_runtime,
                                           conf->prepared_handler,
                                           conf->adapter,
                                           &native_request,
                                           &outcome);
        } else {
            native_request_v3.request = native_request;
            native_request_v3.body = body;
            native_request_v3.max_body_bytes = conf->request_body_max;
            native_request_v3.max_chunk_bytes = conf->request_body_chunk;
            native_request_v3.require_declared_length = 1;
            rc = hoplite_handler_invoke_v3(ngx_http_hoplite_runtime,
                                           conf->prepared_handler,
                                           conf->adapter,
                                           &native_request_v3,
                                           &outcome);
        }
        if (rc != 0) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (outcome.kind == 1) {
            /* send_native borrows response slices until request cleanup. */
            ctx->response = outcome.id;
            return ngx_http_hoplite_send_native(ctx);
        }
        if (outcome.kind != 2 || outcome.id == 0) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        ctx->work = outcome.id;
    }

    ngx_queue_insert_tail(&ngx_http_hoplite_requests, &ctx->queue);
    ctx->queued = 1;
    request->main->count++;
    if (ngx_http_hoplite_drain(request->connection->log) != NGX_OK) {
        ngx_http_hoplite_finish(ctx);
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    return NGX_DONE;
}

static void
ngx_http_hoplite_body_handler(ngx_http_request_t *request)
{
    ngx_http_hoplite_ctx_t *ctx;
    ngx_http_hoplite_loc_conf_t *conf;
    hoplite_request_body_v1 descriptor;
    ngx_chain_t *chain;
    off_t total = 0, size;
    ngx_int_t rc;

    ctx = ngx_http_get_module_ctx(request, ngx_http_hoplite_module);
    conf = ngx_http_get_module_loc_conf(request, ngx_http_hoplite_module);
    if (ctx == NULL || conf == NULL || request->request_body == NULL) {
        ngx_http_finalize_request(request, NGX_HTTP_INTERNAL_SERVER_ERROR);
        return;
    }

    for (chain = request->request_body->bufs;
         chain != NULL;
         chain = chain->next)
    {
        size = ngx_buf_size(chain->buf);
        if (size < 0 || total > request->headers_in.content_length_n - size) {
            ngx_http_finalize_request(request, NGX_HTTP_BAD_REQUEST);
            return;
        }
        total += size;
    }
    if (total != request->headers_in.content_length_n) {
        ngx_http_finalize_request(request, NGX_HTTP_BAD_REQUEST);
        return;
    }

    ctx->body.request = request;
    ctx->body.chain = request->request_body->bufs;
    ctx->body.offset = 0;
    ctx->body.closed = 0;
    descriptor.context = &ctx->body;
    descriptor.declared_length = (uint64_t) total;
    descriptor.has_declared_length = 1;
    descriptor.read = ngx_http_hoplite_body_read;
    descriptor.close = ngx_http_hoplite_body_close;

    rc = ngx_http_hoplite_invoke(request, ctx, conf, &descriptor);
    ngx_http_finalize_request(request, rc);
}

static ngx_int_t
ngx_http_hoplite_handler(ngx_http_request_t *request)
{
    ngx_http_hoplite_loc_conf_t *conf;
    ngx_http_hoplite_ctx_t *ctx;
    ngx_http_cleanup_t *cleanup;
    ngx_int_t rc;

    if (ngx_http_hoplite_runtime == NULL) {
        return NGX_HTTP_SERVICE_UNAVAILABLE;
    }
    conf = ngx_http_get_module_loc_conf(request, ngx_http_hoplite_module);
    if (conf == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    ctx = ngx_pcalloc(request->pool, sizeof(ngx_http_hoplite_ctx_t));
    if (ctx == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    ctx->request = request;
    ngx_queue_init(&ctx->queue);
    ngx_http_set_ctx(request, ctx, ngx_http_hoplite_module);
    cleanup = ngx_http_cleanup_add(request, 0);
    if (cleanup == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    cleanup->handler = ngx_http_hoplite_cleanup;
    cleanup->data = ctx;

    if (!conf->request_body
        || request->headers_in.content_length_n == 0
        || (request->headers_in.content_length_n < 0
            && !request->headers_in.chunked))
    {
        return ngx_http_hoplite_invoke(request, ctx, conf, NULL);
    }
    if (hoplite_abi_version() < 3) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    if (conf->adapter == NGX_HTTP_HOPLITE_REQUEST_HTA) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    if (request->headers_in.content_length_n < 0) {
        return NGX_HTTP_LENGTH_REQUIRED;
    }
    if ((uint64_t) request->headers_in.content_length_n
        > (uint64_t) conf->request_body_max)
    {
        return NGX_HTTP_REQUEST_ENTITY_TOO_LARGE;
    }

    request->request_body_in_clean_file = 1;
    rc = ngx_http_read_client_request_body(request,
                                           ngx_http_hoplite_body_handler);
    if (rc >= NGX_HTTP_SPECIAL_RESPONSE) {
        return rc;
    }
    return NGX_DONE;
}

static ngx_int_t
ngx_http_hoplite_read_file(ngx_cycle_t *cycle, const ngx_str_t *path,
                           ngx_str_t *source, ngx_flag_t append_nil)
{
    ngx_file_t file;
    ngx_file_info_t info;
    ssize_t read;

    ngx_memzero(&file, sizeof(file));
    file.name = *path;
    file.log = cycle->log;
    file.fd = ngx_open_file(path->data, NGX_FILE_RDONLY, NGX_FILE_OPEN, 0);
    if (file.fd == NGX_INVALID_FILE) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, ngx_errno,
                      "hoplite could not open bootstrap file %V", path);
        return NGX_ERROR;
    }
    if (ngx_fd_info(file.fd, &info) == NGX_FILE_ERROR) {
        ngx_close_file(file.fd);
        return NGX_ERROR;
    }

    source->len = (size_t) ngx_file_size(&info);
    source->data = ngx_alloc(source->len + (append_nil ? sizeof("\nnil") - 1 : 0), cycle->log);
    if (source->data == NULL) {
        ngx_close_file(file.fd);
        return NGX_ERROR;
    }
    read = ngx_read_file(&file, source->data, source->len, 0);
    ngx_close_file(file.fd);
    if (read == NGX_ERROR || (size_t) read != source->len) {
        ngx_free(source->data);
        source->data = NULL;
        return NGX_ERROR;
    }
    if (append_nil) {
        ngx_memcpy(source->data + source->len, "\nnil", sizeof("\nnil") - 1);
        source->len += sizeof("\nnil") - 1;
    }
    return NGX_OK;
}

static ngx_int_t
ngx_http_hoplite_bootstrap(ngx_cycle_t *cycle, const ngx_str_t *path)
{
    ngx_str_t source;
    hoplite_buffer_t buffer;
    hoplite_hta_value_t *event;
    ngx_pool_t *pool;
    int64_t kind, event_work;
    uint64_t work;
    ngx_int_t rc = NGX_ERROR;

    if (ngx_http_hoplite_read_file(cycle, path, &source, 1) != NGX_OK) {
        return NGX_ERROR;
    }
    work = hoplite_work_start(ngx_http_hoplite_runtime,
                         source.data, source.len, NULL, 0);
    ngx_free(source.data);
    if (work == 0 || hoplite_work_poll(ngx_http_hoplite_runtime) == 0) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite bootstrap suspended or failed to start");
        return NGX_ERROR;
    }

    while (hoplite_work_poll(ngx_http_hoplite_runtime) != 0) {
        if (hoplite_work_next_event(ngx_http_hoplite_runtime, &buffer) != 0) {
            break;
        }
        pool = ngx_create_pool(4096, cycle->log);
        if (pool == NULL) {
            hoplite_buffer_free(buffer.data, buffer.len);
            break;
        }
        if (hoplite_hta_decode(pool, buffer.data, buffer.len, &event) == NGX_OK
            && event->kind == HOPLITE_HTA_VECTOR
            && event->as.vector.count == 3
            && hoplite_hta_number(event->as.vector.items[0], &kind) == NGX_OK
            && hoplite_hta_number(event->as.vector.items[1], &event_work) == NGX_OK
            && (uint64_t) event_work == work)
        {
            rc = kind == 0 ? NGX_OK : NGX_ERROR;
        }
        ngx_destroy_pool(pool);
        hoplite_buffer_free(buffer.data, buffer.len);
    }
    (void) hoplite_work_close(ngx_http_hoplite_runtime, work);

    if (rc != NGX_OK) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite bootstrap evaluation failed");
    }
    return rc;
}

static ngx_int_t
ngx_http_hoplite_init_process(ngx_cycle_t *cycle)
{
    ngx_http_hoplite_main_conf_t *conf;
    int32_t value_store_status;
    int32_t blob_status;

    ngx_queue_init(&ngx_http_hoplite_requests);
    ngx_http_hoplite_queue_ready = 1;
    hoplite_host_registry_init(&ngx_http_hoplite_providers);
    if (ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_nginx_provider) != NGX_OK)
    {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite native host providers could not be registered");
        return NGX_ERROR;
    }
    value_store_status = hoplite_value_store_host_provider_init_process_v1();
    if (value_store_status == HOPLITE_VALUE_STORE_HOST_PROVIDER_ERROR) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite hara.store provider could not be initialized");
        return NGX_ERROR;
    }
    blob_status = hoplite_blob_host_provider_init_process_v1();
    if (blob_status == HOPLITE_BLOB_HOST_PROVIDER_ERROR) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite hara.blob provider could not be initialized");
        return NGX_ERROR;
    }
    ngx_http_hoplite_runtime = hoplite_runtime_new();
    if (ngx_http_hoplite_runtime == NULL || hoplite_abi_version() < 2) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite runtime could not be initialized");
        return NGX_ERROR;
    }

    conf = ngx_http_cycle_get_module_main_conf(cycle, ngx_http_hoplite_module);
    if (conf != NULL && conf->bootstrap.len != 0
        && ngx_http_hoplite_bootstrap(cycle, &conf->bootstrap) != NGX_OK)
    {
        return NGX_ERROR;
    }
    if (conf != NULL && conf->manifest.len != 0) {
        ngx_str_t manifest;
        if (ngx_http_hoplite_read_file(cycle, &conf->manifest, &manifest, 0) != NGX_OK) {
            return NGX_ERROR;
        }
        if (hoplite_apps_prepare(ngx_http_hoplite_runtime,
                                 manifest.data, manifest.len) != 0) {
            ngx_free(manifest.data);
            ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                          "hoplite could not prepare app manifest %V", &conf->manifest);
            return NGX_ERROR;
        }
        ngx_free(manifest.data);
    }
    return NGX_OK;
}

static void
ngx_http_hoplite_exit_process(ngx_cycle_t *cycle)
{
    (void) cycle;
    if (ngx_http_hoplite_runtime != NULL) {
        hoplite_runtime_free(ngx_http_hoplite_runtime);
        ngx_http_hoplite_runtime = NULL;
    }
    hoplite_blob_host_provider_exit_process_v1();
    hoplite_value_store_host_provider_exit_process_v1();
    ngx_http_hoplite_queue_ready = 0;
}

static void *
ngx_http_hoplite_create_main_conf(ngx_conf_t *cf)
{
    return ngx_pcalloc(cf->pool, sizeof(ngx_http_hoplite_main_conf_t));
}

static void *
ngx_http_hoplite_create_loc_conf(ngx_conf_t *cf)
{
    ngx_http_hoplite_loc_conf_t *conf;
    conf = ngx_pcalloc(cf->pool, sizeof(ngx_http_hoplite_loc_conf_t));
    if (conf != NULL) {
        conf->app = NGX_CONF_UNSET_UINT;
        conf->adapter = NGX_CONF_UNSET_UINT;
        conf->request_body = NGX_CONF_UNSET;
        conf->request_body_max = NGX_CONF_UNSET_SIZE;
        conf->request_body_chunk = NGX_CONF_UNSET_SIZE;
        conf->response_body_chunk = NGX_CONF_UNSET_SIZE;
    }
    return conf;
}

static char *
ngx_http_hoplite_merge_loc_conf(ngx_conf_t *cf, void *parent, void *child)
{
    ngx_http_hoplite_loc_conf_t *previous = parent;
    ngx_http_hoplite_loc_conf_t *conf = child;

    ngx_conf_merge_str_value(conf->handler, previous->handler, "");
    ngx_conf_merge_uint_value(conf->app, previous->app, NGX_CONF_UNSET_UINT);
    ngx_conf_merge_uint_value(conf->adapter, previous->adapter,
                              NGX_HTTP_HOPLITE_REQUEST);
    ngx_conf_merge_value(conf->request_body, previous->request_body, 0);
    ngx_conf_merge_size_value(conf->request_body_max,
                              previous->request_body_max, 8 * 1024 * 1024);
    ngx_conf_merge_size_value(conf->request_body_chunk,
                              previous->request_body_chunk, 64 * 1024);
    ngx_conf_merge_size_value(conf->response_body_chunk,
                              previous->response_body_chunk, 64 * 1024);
    if (conf->request_body_chunk == 0
        || conf->request_body_max == 0
        || conf->request_body_chunk > conf->request_body_max)
    {
        return "hoplite request body limits must be positive and chunk <= max";
    }
    if (conf->response_body_chunk == 0) {
        return "hoplite response body chunk must be positive";
    }
    if (conf->request_body
        && conf->adapter == NGX_HTTP_HOPLITE_REQUEST_HTA)
    {
        return "hoplite_request_body cannot be used with request+hta";
    }
    (void) cf;
    return NGX_CONF_OK;
}

static char *
ngx_http_hoplite_app(ngx_conf_t *cf, ngx_command_t *cmd, void *conf)
{
    ngx_http_hoplite_loc_conf_t *location = conf;
    ngx_http_core_loc_conf_t *core;
    ngx_str_t *value;
    ngx_int_t app;

    (void) cmd;
    if (location->app != NGX_CONF_UNSET_UINT || location->handler.len != 0) {
        return "is duplicate";
    }
    value = cf->args->elts;
    app = ngx_atoi(value[1].data, value[1].len);
    if (app <= 0) {
        return "must be a positive app id";
    }
    location->app = (ngx_uint_t) app;
    core = ngx_http_conf_get_module_loc_conf(cf, ngx_http_core_module);
    core->handler = ngx_http_hoplite_handler;
    return NGX_CONF_OK;
}

static char *
ngx_http_hoplite_content(ngx_conf_t *cf, ngx_command_t *cmd, void *conf)
{
    ngx_http_hoplite_loc_conf_t *location = conf;
    ngx_http_core_loc_conf_t *core;
    ngx_str_t *value;

    (void) cmd;
    if (location->handler.len != 0) {
        return "is duplicate";
    }
    value = cf->args->elts;
    location->handler = value[1];
    location->adapter = NGX_HTTP_HOPLITE_REQUEST;
    if (cf->args->nelts == 3) {
        if (ngx_strcmp(value[2].data, "raw") == 0) {
            location->adapter = NGX_HTTP_HOPLITE_RAW;
        } else if (ngx_strcmp(value[2].data, "request") == 0) {
            location->adapter = NGX_HTTP_HOPLITE_REQUEST;
        } else if (ngx_strcmp(value[2].data, "request+hta") == 0) {
            location->adapter = NGX_HTTP_HOPLITE_REQUEST_HTA;
        } else {
            return "adapter must be raw, request, or request+hta";
        }
    }
    core = ngx_http_conf_get_module_loc_conf(cf, ngx_http_core_module);
    core->handler = ngx_http_hoplite_handler;
    return NGX_CONF_OK;
}