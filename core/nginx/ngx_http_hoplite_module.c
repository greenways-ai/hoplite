#include <ngx_config.h>
#include <ngx_core.h>
#include <ngx_http.h>

#include "hoplite_hta.h"
#include "hoplite_response_source.h"
#include "hoplite_host_provider.h"
#include "hoplite_runtime.h"

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
    ngx_msec_t request_timeout;
} ngx_http_hoplite_loc_conf_t;

#define NGX_HTTP_HOPLITE_RAW 0
#define NGX_HTTP_HOPLITE_REQUEST 1
#define NGX_HTTP_HOPLITE_REQUEST_HTA 2
#define NGX_HTTP_HOPLITE_RESPONSE_SOURCE_CHUNK (64u * 1024u)

typedef struct {
    ngx_http_request_t *request;
    ngx_chain_t *chain;
    off_t offset;
    ngx_flag_t closed;
} ngx_http_hoplite_body_t;

typedef struct {
    hoplite_response_source_state_v1_t source;
    ngx_buf_t *buffer;
    ngx_chain_t chain;
    u_char *storage;
    size_t capacity;
    ngx_flag_t active;
    ngx_flag_t submitted;
    ngx_flag_t last;
    ngx_flag_t native_stream;
} ngx_http_hoplite_response_source_t;

typedef struct ngx_http_hoplite_ctx_s ngx_http_hoplite_ctx_t;
typedef struct ngx_http_hoplite_native_completion_s
    ngx_http_hoplite_native_completion_t;

typedef struct {
    ngx_event_t event;
    ngx_http_hoplite_ctx_t *ctx;
    uint64_t call;
} ngx_http_hoplite_sleep_t;


typedef struct ngx_http_hoplite_provider_s ngx_http_hoplite_provider_t;
typedef struct ngx_http_hoplite_rtc_session_s ngx_http_hoplite_rtc_session_t;

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

struct ngx_http_hoplite_rtc_session_s {
    uint64_t id;
    hoplite_rtc_engine_t *engine;
    ngx_connection_t *udp;
    ngx_event_t timer;
    ngx_http_hoplite_ctx_t *receiver;
    uint64_t receive_call;
    hoplite_buffer_t received;
    ngx_http_hoplite_rtc_session_t *next;
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
    const hoplite_host_provider_v1_t *response_provider;
    ngx_http_hoplite_native_completion_t *native_completion;
    ngx_http_hoplite_body_t body;
    ngx_http_hoplite_response_source_t response_source;
    ngx_event_t timeout;
};

static void
ngx_http_hoplite_request_failure(ngx_log_t *log, const char *failure_class)
{
    ngx_log_error(
        NGX_LOG_ERR, log, 0,
        "hoplite request failure: {\"format\":\"hoplite.request-failure/0-alpha\",\"class\":\"%s\"}",
        failure_class);
}

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
static ngx_http_hoplite_rtc_session_t *ngx_http_hoplite_rtc_sessions;
static uint64_t ngx_http_hoplite_rtc_next_id;

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
static void ngx_http_hoplite_response_source_write_handler(
    ngx_http_request_t *request);
static void ngx_http_hoplite_response_source_release(
    ngx_http_hoplite_ctx_t *ctx);

static ngx_int_t ngx_http_hoplite_provider_register(
    const ngx_http_hoplite_provider_t *provider);
static const ngx_http_hoplite_provider_t *ngx_http_hoplite_provider_find(
    ngx_str_t service);
static ngx_int_t ngx_http_hoplite_nginx_invoke(
    const ngx_http_hoplite_host_call_t *call);
static void ngx_http_hoplite_nginx_cancel(ngx_http_hoplite_ctx_t *ctx);
static ngx_int_t ngx_http_hoplite_rtc_invoke(
    const ngx_http_hoplite_host_call_t *call);
static void ngx_http_hoplite_rtc_cancel(ngx_http_hoplite_ctx_t *ctx);
static void ngx_http_hoplite_rtc_clear(void);
static ngx_int_t ngx_http_hoplite_rtc_pump(
    ngx_http_hoplite_rtc_session_t *session, ngx_log_t *log);
static void ngx_http_hoplite_rtc_read_handler(ngx_event_t *event);
static void ngx_http_hoplite_rtc_timer_handler(ngx_event_t *event);
static void ngx_http_hoplite_rtc_session_free(
    ngx_http_hoplite_rtc_session_t *session);

static const ngx_http_hoplite_provider_t ngx_http_hoplite_nginx_provider = {
    {(const uint8_t *) "nginx", sizeof("nginx") - 1},
    ngx_http_hoplite_nginx_invoke,
    ngx_http_hoplite_nginx_cancel,
    0
};

static const ngx_http_hoplite_provider_t ngx_http_hoplite_rtc_provider = {
    {(const uint8_t *) "hoplite.rtc", sizeof("hoplite.rtc") - 1},
    ngx_http_hoplite_rtc_invoke,
    ngx_http_hoplite_rtc_cancel,
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
        ngx_string("hoplite_request_timeout"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_msec_slot,
        NGX_HTTP_LOC_CONF_OFFSET,
        offsetof(ngx_http_hoplite_loc_conf_t, request_timeout),
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
ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)
{
    if (ctx->done) {
        return;
    }
    if (ctx->timeout.timer_set) {
        ngx_del_timer(&ctx->timeout);
    }
    ngx_http_hoplite_response_source_release(ctx);
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
    if (ngx_http_hoplite_runtime != NULL && ctx->work != 0) {
        if (hoplite_work_close(ngx_http_hoplite_runtime, ctx->work) != 0
            && ctx->request != NULL)
        {
            ngx_http_hoplite_request_failure(
                ctx->request->connection->log, "cleanup");
        }
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
ngx_http_hoplite_response_source_name(const hoplite_hta_value_t *value,
                                      const char *name)
{
    size_t len;

    if (value == NULL || name == NULL
        || value->kind != HOPLITE_HTA_KEYWORD)
    {
        return 0;
    }
    len = ngx_strlen(name);
    return value->as.text.len == len
        && ngx_strncmp(value->as.text.data, name, len) == 0;
}

static ngx_int_t
ngx_http_hoplite_response_source_parse(
    const hoplite_hta_value_t *value,
    hoplite_response_source_descriptor_v1_t *descriptor)
{
    size_t i;
    ngx_uint_t seen = 0;
    int64_t number;
    const hoplite_hta_pair_t *pair;

    if (value == NULL || descriptor == NULL
        || value->kind != HOPLITE_HTA_MAP
        || value->as.map.count != 5)
    {
        return NGX_ERROR;
    }

    ngx_memzero(descriptor, sizeof(*descriptor));
    for (i = 0; i < value->as.map.count; i++) {
        pair = &value->as.map.items[i];
        if (ngx_http_hoplite_response_source_name(pair->key, "protocol")) {
            if ((seen & 1u) != 0
                || pair->value == NULL
                || pair->value->kind != HOPLITE_HTA_STRING)
            {
                return NGX_ERROR;
            }
            descriptor->protocol = pair->value->as.text.data;
            descriptor->protocol_len = pair->value->as.text.len;
            seen |= 1u;

        } else if (ngx_http_hoplite_response_source_name(pair->key, "service")) {
            if ((seen & 2u) != 0
                || pair->value == NULL
                || pair->value->kind != HOPLITE_HTA_STRING
                || pair->value->as.text.len == 0)
            {
                return NGX_ERROR;
            }
            descriptor->service = pair->value->as.text.data;
            descriptor->service_len = pair->value->as.text.len;
            seen |= 2u;

        } else if (ngx_http_hoplite_response_source_name(
                       pair->key, "source-handle"))
        {
            if ((seen & 4u) != 0
                || hoplite_hta_number(pair->value, &number) != NGX_OK
                || number < 0)
            {
                return NGX_ERROR;
            }
            descriptor->source_handle = (uint64_t) number;
            seen |= 4u;

        } else if (ngx_http_hoplite_response_source_name(
                       pair->key, "offset"))
        {
            if ((seen & 8u) != 0
                || hoplite_hta_number(pair->value, &number) != NGX_OK
                || number < 0)
            {
                return NGX_ERROR;
            }
            descriptor->offset = (uint64_t) number;
            seen |= 8u;

        } else if (ngx_http_hoplite_response_source_name(
                       pair->key, "length"))
        {
            if ((seen & 16u) != 0
                || hoplite_hta_number(pair->value, &number) != NGX_OK
                || number < 0)
            {
                return NGX_ERROR;
            }
            descriptor->length = (uint64_t) number;
            seen |= 16u;

        } else {
            return NGX_ERROR;
        }
    }

    return seen == 31u
        && hoplite_response_source_descriptor_validate_v1(descriptor)
               == HOPLITE_RESPONSE_SOURCE_OK
        ? NGX_OK
        : NGX_ERROR;
}

static int32_t
ngx_http_hoplite_response_source_read(void *request_context,
                                      uint64_t work,
                                      uint64_t source_handle,
                                      uint8_t *output,
                                      size_t capacity,
                                      size_t *returned)
{
    ngx_http_hoplite_ctx_t *ctx = request_context;

    return ctx != NULL && ctx->response_provider != NULL
        && ctx->response_provider->response_read != NULL
        && ctx->response_provider->response_read(
               request_context, work, source_handle,
               output, capacity, returned) == HOPLITE_HOST_PROVIDER_OK
        ? HOPLITE_RESPONSE_SOURCE_OK
        : HOPLITE_RESPONSE_SOURCE_ERROR;
}

static int32_t
ngx_http_hoplite_response_source_close(void *request_context,
                                       uint64_t work,
                                       uint64_t source_handle)
{
    ngx_http_hoplite_ctx_t *ctx = request_context;

    return ctx != NULL && ctx->response_provider != NULL
        && ctx->response_provider->response_close != NULL
        && ctx->response_provider->response_close(
               request_context, work, source_handle)
               == HOPLITE_HOST_PROVIDER_OK
        ? HOPLITE_RESPONSE_SOURCE_OK
        : HOPLITE_RESPONSE_SOURCE_ERROR;
}

static void
ngx_http_hoplite_response_source_release(ngx_http_hoplite_ctx_t *ctx)
{
    if (ctx == NULL) {
        return;
    }
    ctx->response_source.active = 0;
    (void) hoplite_response_source_close_v1(&ctx->response_source.source);
    if (ctx->response_provider != NULL
        && ctx->response_provider->release_work != NULL
        && ctx->work != 0)
    {
        ctx->response_provider->release_work(ctx->work);
    }
    ctx->response_provider = NULL;
}

static ngx_int_t
ngx_http_hoplite_response_source_arm(ngx_http_request_t *request)
{
    ngx_event_t *write;
    ngx_http_core_loc_conf_t *configuration;

    write = request->connection->write;
    request->http_state = NGX_HTTP_WRITING_REQUEST_STATE;
    request->read_event_handler = ngx_http_test_reading;
    request->write_event_handler =
        ngx_http_hoplite_response_source_write_handler;

    if (write->ready && write->delayed) {
        return NGX_OK;
    }

    configuration =
        ngx_http_get_module_loc_conf(request, ngx_http_core_module);
    if (!write->delayed && !write->timer_set) {
        ngx_add_timer(write, configuration->send_timeout);
    }
    return ngx_handle_write_event(write, configuration->send_lowat);
}

static ngx_int_t
ngx_http_hoplite_response_source_fill(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_response_source_t *stream;
    size_t returned = 0;
    uint8_t last = 0;
    int32_t rc;

    stream = &ctx->response_source;
    stream->buffer->pos = stream->storage;
    stream->buffer->last = stream->storage;
    stream->buffer->flush = 0;
    stream->buffer->last_buf = 0;
    stream->buffer->last_in_chain = 0;
    stream->submitted = 0;
    stream->last = 0;

    if (stream->native_stream) {
        hoplite_slice_t chunk;
        rc = hoplite_response_stream_next_v3(
            ngx_http_hoplite_runtime, ctx->response, &chunk);
        if (rc == 1) {
            return NGX_AGAIN;
        }
        if (rc == 2) {
            stream->buffer->last_buf = 1;
            stream->buffer->last_in_chain = 1;
            stream->last = 1;
            return NGX_OK;
        }
        if (rc != 0 || chunk.len == 0 || chunk.len > stream->capacity) {
            return NGX_ERROR;
        }
        ngx_memcpy(stream->storage, chunk.data, chunk.len);
        returned = chunk.len;
    } else {
        rc = hoplite_response_source_next_v1(
            &stream->source,
            stream->storage,
            stream->capacity,
            &returned,
            &last);
    }
    if (rc != HOPLITE_RESPONSE_SOURCE_OK || returned == 0) {
        return NGX_ERROR;
    }

    stream->buffer->last += returned;
    stream->buffer->flush = last ? 0 : 1;
    stream->buffer->last_buf = last;
    stream->buffer->last_in_chain = last;
    stream->last = last;
    return NGX_OK;
}

static void
ngx_http_hoplite_response_source_abort(ngx_http_hoplite_ctx_t *ctx,
                                       ngx_int_t rc)
{
    ngx_http_request_t *request;
    ngx_str_t body = ngx_string("Hoplite response source failed\n");

    if (ctx == NULL || ctx->done) {
        return;
    }
    request = ctx->request;
    ngx_http_hoplite_request_failure(
        request->connection->log, "response-stream");
    ngx_http_hoplite_response_source_release(ctx);
    ctx->response_source.active = 0;

    if (!request->header_sent) {
        ngx_http_hoplite_send(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
        return;
    }

    request->connection->error = 1;
    ngx_http_hoplite_finish(ctx);
    ngx_http_finalize_request(request, rc);
}

static ngx_int_t
ngx_http_hoplite_response_source_drive(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_request_t *request;
    ngx_http_hoplite_response_source_t *stream;
    ngx_connection_t *connection;
    ngx_int_t rc;

    request = ctx->request;
    stream = &ctx->response_source;
    connection = request->connection;

    for ( ;; ) {
        if (!stream->active || ctx->done) {
            return NGX_OK;
        }

        if (stream->submitted) {
            rc = ngx_http_output_filter(request, NULL);
        } else {
            rc = ngx_http_output_filter(request, &stream->chain);
            stream->submitted = 1;
        }
        if (rc == NGX_ERROR) {
            ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
            return NGX_ERROR;
        }

        if (rc == NGX_AGAIN
            || stream->buffer->pos != stream->buffer->last
            || request->buffered
            || request->postponed
            || (request == request->main && connection->buffered))
        {
            if (ngx_http_hoplite_response_source_arm(request) != NGX_OK) {
                ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
                return NGX_ERROR;
            }
            return NGX_AGAIN;
        }

        if (stream->last) {
            stream->active = 0;
            ngx_http_hoplite_finish(ctx);
            ngx_http_finalize_request(request, NGX_OK);
            return NGX_OK;
        }

        rc = ngx_http_hoplite_response_source_fill(ctx);
        if (rc == NGX_AGAIN) {
            if (ngx_http_hoplite_response_source_arm(request) != NGX_OK) {
                ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
                return NGX_ERROR;
            }
            return NGX_AGAIN;
        }
        if (rc != NGX_OK) {
            ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
            return NGX_ERROR;
        }
    }
}

static ngx_int_t
ngx_http_hoplite_native_stream_start(ngx_http_hoplite_ctx_t *ctx,
                                     uint16_t status)
{
    ngx_http_request_t *request = ctx->request;
    ngx_http_hoplite_response_source_t *stream = &ctx->response_source;
    hoplite_slice_t key, value;
    ngx_table_elt_t *header;
    size_t count, i;
    ngx_int_t rc;

    count = hoplite_response_header_count_v2(ngx_http_hoplite_runtime,
                                              ctx->response);
    for (i = 0; i < count; i++) {
        if (hoplite_response_header_at_v2(ngx_http_hoplite_runtime,
                                          ctx->response, i,
                                          &key, &value) != 0) return NGX_ERROR;
        if (key.len == sizeof("content-length") - 1
            && ngx_strncasecmp((u_char *) key.data,
                               (u_char *) "content-length", key.len) == 0) continue;
        if (key.len == sizeof("content-type") - 1
            && ngx_strncasecmp((u_char *) key.data,
                               (u_char *) "content-type", key.len) == 0) {
            request->headers_out.content_type.data = (u_char *) value.data;
            request->headers_out.content_type.len = value.len;
            continue;
        }
        header = ngx_list_push(&request->headers_out.headers);
        if (header == NULL) return NGX_ERROR;
        header->hash = 1;
        header->key.data = (u_char *) key.data;
        header->key.len = key.len;
        header->value.data = (u_char *) value.data;
        header->value.len = value.len;
    }

    stream->native_stream = 1;
    if (request->header_only) {
        request->headers_out.status = status;
        request->headers_out.content_length_n = -1;
        rc = ngx_http_send_header(request);
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, rc);
        return NGX_OK;
    }
    stream->capacity = NGX_HTTP_HOPLITE_RESPONSE_SOURCE_CHUNK;
    stream->storage = ngx_pnalloc(request->pool, stream->capacity);
    stream->buffer = ngx_calloc_buf(request->pool);
    if (stream->storage == NULL || stream->buffer == NULL) return NGX_ERROR;
    stream->buffer->start = stream->storage;
    stream->buffer->end = stream->storage + stream->capacity;
    stream->buffer->temporary = 1;
    stream->buffer->tag = (ngx_buf_tag_t) &ngx_http_hoplite_module;
    stream->chain.buf = stream->buffer;
    stream->chain.next = NULL;
    request->headers_out.status = status;
    request->headers_out.content_length_n = -1;
    if (request->headers_out.content_type.len == 0) {
        ngx_str_set(&request->headers_out.content_type, "application/octet-stream");
    }
    stream->active = 1;
    rc = ngx_http_send_header(request);
    if (rc == NGX_ERROR || rc > NGX_OK) return rc;
    rc = ngx_http_hoplite_response_source_fill(ctx);
    if (rc == NGX_ERROR) return NGX_ERROR;
    if (rc == NGX_AGAIN) return ngx_http_hoplite_response_source_arm(request);
    return ngx_http_hoplite_response_source_drive(ctx);
}

static void
ngx_http_hoplite_response_source_write_handler(ngx_http_request_t *request)
{
    ngx_http_hoplite_ctx_t *ctx;
    ngx_event_t *write;
    ngx_connection_t *connection;

    ctx = ngx_http_get_module_ctx(request, ngx_http_hoplite_module);
    if (ctx == NULL || ctx->done || !ctx->response_source.active) {
        return;
    }

    connection = request->connection;
    write = connection->write;
    if (write->timedout) {
        ngx_log_error(NGX_LOG_INFO, connection->log, NGX_ETIMEDOUT,
                      "client timed out while streaming a Hoplite response source");
        connection->timedout = 1;
        ngx_http_hoplite_response_source_abort(
            ctx, NGX_HTTP_REQUEST_TIME_OUT);
        return;
    }
    if (connection->error) {
        ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
        return;
    }
    if (write->delayed) {
        if (ngx_http_hoplite_response_source_arm(request) != NGX_OK) {
            ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
        }
        return;
    }

    (void) ngx_http_hoplite_response_source_drive(ctx);
}

static void
ngx_http_hoplite_response_source_start(
    ngx_http_hoplite_ctx_t *ctx,
    ngx_uint_t status,
    const hoplite_hta_value_t *headers,
    const hoplite_response_source_descriptor_v1_t *descriptor)
{
    ngx_http_request_t *request;
    ngx_http_hoplite_response_source_t *stream;
    hoplite_host_service_t service;
    ngx_int_t rc;

    request = ctx->request;
    stream = &ctx->response_source;
    service.data = descriptor->service;
    service.len = descriptor->service_len;
    ctx->response_provider = hoplite_host_provider_find_v1(service);
    if (ctx->work == 0
        || ctx->response_provider == NULL
        || (ctx->response_provider->capabilities
            & HOPLITE_HOST_PROVIDER_RESPONSE_BODY) == 0
        || stream->source.initialized
        || descriptor->length > (uint64_t) NGX_MAX_OFF_T_VALUE
        || hoplite_response_source_init_v1(
               &stream->source,
               ctx,
               ctx->work,
               descriptor,
               ngx_http_hoplite_response_source_read,
               ngx_http_hoplite_response_source_close)
               != HOPLITE_RESPONSE_SOURCE_OK)
    {
        ngx_http_hoplite_response_source_abort(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR);
        return;
    }

    if (request->header_only) {
        if (hoplite_response_source_close_v1(&stream->source)
                != HOPLITE_RESPONSE_SOURCE_OK
            || ngx_http_hoplite_add_headers(request, headers) != NGX_OK)
        {
            ngx_http_hoplite_response_source_abort(
                ctx, NGX_HTTP_INTERNAL_SERVER_ERROR);
            return;
        }

        request->headers_out.status = status;
        request->headers_out.content_length_n = (off_t) descriptor->length;
        if (request->headers_out.content_type.len == 0) {
            ngx_str_set(&request->headers_out.content_type,
                        "application/octet-stream");
        }
        rc = ngx_http_send_header(request);
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, rc);
        return;
    }

    stream->capacity = NGX_HTTP_HOPLITE_RESPONSE_SOURCE_CHUNK;
    stream->storage = ngx_pnalloc(request->pool, stream->capacity);
    stream->buffer = ngx_calloc_buf(request->pool);
    if (stream->storage == NULL || stream->buffer == NULL) {
        ngx_http_hoplite_response_source_abort(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR);
        return;
    }

    stream->buffer->start = stream->storage;
    stream->buffer->end = stream->storage + stream->capacity;
    stream->buffer->temporary = 1;
    stream->buffer->tag = (ngx_buf_tag_t) &ngx_http_hoplite_module;
    stream->chain.buf = stream->buffer;
    stream->chain.next = NULL;
    if (ngx_http_hoplite_response_source_fill(ctx) != NGX_OK
        || ngx_http_hoplite_add_headers(request, headers) != NGX_OK)
    {
        ngx_http_hoplite_response_source_abort(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR);
        return;
    }

    request->headers_out.status = status;
    request->headers_out.content_length_n = (off_t) descriptor->length;
    if (request->headers_out.content_type.len == 0) {
        ngx_str_set(&request->headers_out.content_type,
                    "application/octet-stream");
    }

    stream->active = 1;
    rc = ngx_http_send_header(request);
    if (rc == NGX_ERROR) {
        ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
        return;
    }
    if (rc > NGX_OK) {
        stream->active = 0;
        ngx_http_hoplite_finish(ctx);
        ngx_http_finalize_request(request, rc);
        return;
    }

    (void) ngx_http_hoplite_response_source_drive(ctx);
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
                                   &status) != 0)
    {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    if (hoplite_response_body_kind_v3(ngx_http_hoplite_runtime,
                                      ctx->response) == 1) {
        return ngx_http_hoplite_native_stream_start(ctx, status);
    }
    if (hoplite_response_body_v2(ngx_http_hoplite_runtime, ctx->response,
                                 &body) != 0) return NGX_HTTP_INTERNAL_SERVER_ERROR;

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
    hoplite_response_source_descriptor_v1_t descriptor;
    int64_t status_number = NGX_HTTP_OK;
    ngx_flag_t status_valid = 1;
    ngx_str_t body = ngx_null_string;

    if (payload == NULL
        || (payload->kind != HOPLITE_HTA_MAP
            && payload->kind != HOPLITE_HTA_OBJECT))
    {
        ngx_str_set(&body, "Hoplite handler must return a response map\n");
        ngx_http_hoplite_send(
            ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
        return;
    }

    status_value = hoplite_hta_map_get(payload, "status");
    body_value = hoplite_hta_map_get(payload, "body");
    headers = hoplite_hta_map_get(payload, "headers");

    if (status_value != NULL
        && hoplite_hta_number(status_value, &status_number) != NGX_OK)
    {
        status_valid = 0;
    }
    if (status_number < 100 || status_number > 599) {
        status_valid = 0;
    }
    if (!status_valid) {
        status_number = NGX_HTTP_INTERNAL_SERVER_ERROR;
        headers = NULL;
        ngx_str_set(&body, "Hoplite handler returned an invalid status\n");
    }

    if (status_valid
        && body_value != NULL
        && body_value->kind == HOPLITE_HTA_MAP)
    {
        if (ngx_http_hoplite_response_source_parse(
                body_value, &descriptor) != NGX_OK)
        {
            ngx_str_set(
                &body,
                "Hoplite response source must be an exact hoplite.response-source/0-alpha map\n");
            ngx_http_hoplite_send(
                ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
            return;
        }

        ngx_http_hoplite_response_source_start(
            ctx, (ngx_uint_t) status_number, headers, &descriptor);
        return;
    }

    if (status_valid
        && body_value != NULL
        && body_value->kind != HOPLITE_HTA_NIL
        && hoplite_hta_text(body_value, &body) != NGX_OK)
    {
        ngx_str_set(
            &body,
            "Hoplite response body must be a string, bytes, or response source\n");
        status_number = NGX_HTTP_INTERNAL_SERVER_ERROR;
        headers = NULL;
    }

    ngx_http_hoplite_send(
        ctx, (ngx_uint_t) status_number, &body, headers);
}

static void
ngx_http_hoplite_send_error(ngx_http_hoplite_ctx_t *ctx,
                            const hoplite_hta_value_t *payload)
{
    ngx_str_t message;

    (void) payload;
    ngx_http_hoplite_request_failure(
        ctx->request->connection->log, "application");

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

static ngx_flag_t
ngx_http_hoplite_operation(const ngx_str_t *actual, const char *expected)
{
    size_t len = ngx_strlen(expected);
    return actual->len == len
        && ngx_strncmp(actual->data, expected, len) == 0;
}

static ngx_http_hoplite_rtc_session_t *
ngx_http_hoplite_rtc_find(uint64_t id)
{
    ngx_http_hoplite_rtc_session_t *session;
    for (session = ngx_http_hoplite_rtc_sessions;
         session != NULL; session = session->next)
    {
        if (session->id == id) {
            return session;
        }
    }
    return NULL;
}

static ngx_int_t
ngx_http_hoplite_rtc_handle(const ngx_http_hoplite_host_call_t *call,
                            size_t count,
                            ngx_http_hoplite_rtc_session_t **session)
{
    int64_t id;
    if (call->arguments == NULL
        || call->arguments->kind != HOPLITE_HTA_VECTOR
        || call->arguments->as.vector.count != count
        || hoplite_hta_number(call->arguments->as.vector.items[0], &id) != NGX_OK
        || id <= 0)
    {
        return NGX_ERROR;
    }
    *session = ngx_http_hoplite_rtc_find((uint64_t) id);
    return *session == NULL ? NGX_ERROR : NGX_OK;
}

static ngx_int_t
ngx_http_hoplite_rtc_resolve_buffer(const ngx_http_hoplite_host_call_t *call,
                                    hoplite_buffer_t *buffer)
{
    ngx_str_t value, encoded;
    ngx_int_t rc;
    value.data = buffer->data;
    value.len = buffer->len;
    rc = hoplite_hta_encode_string(call->pool, &value, &encoded) == NGX_OK
        && hoplite_call_resolve(ngx_http_hoplite_runtime, call->call,
                                encoded.data, encoded.len) == 0
        ? NGX_OK : NGX_ERROR;
    hoplite_buffer_free(buffer->data, buffer->len);
    buffer->data = NULL;
    buffer->len = 0;
    return rc;
}

static void
ngx_http_hoplite_rtc_buffer_free(hoplite_buffer_t *buffer)
{
    if (buffer->data != NULL) {
        hoplite_buffer_free(buffer->data, buffer->len);
        buffer->data = NULL;
        buffer->len = 0;
    }
}

static ngx_int_t
ngx_http_hoplite_rtc_deliver(ngx_http_hoplite_rtc_session_t *session,
                             hoplite_buffer_t *buffer, ngx_log_t *log)
{
    ngx_http_hoplite_ctx_t *ctx = session->receiver;
    ngx_str_t value, encoded;
    uint64_t call = session->receive_call;

    if (ctx == NULL || ctx->done || ctx->provider != &ngx_http_hoplite_rtc_provider) {
        return NGX_DECLINED;
    }
    value.data = buffer->data;
    value.len = buffer->len;
    if (hoplite_hta_encode_string(ctx->request->pool, &value, &encoded) != NGX_OK) {
        return NGX_ERROR;
    }
    session->receiver = NULL;
    session->receive_call = 0;
    ctx->provider = NULL;
    ngx_http_hoplite_rtc_buffer_free(buffer);
    if (hoplite_call_resolve(ngx_http_hoplite_runtime, call,
                             encoded.data, encoded.len) != 0
        || ngx_http_hoplite_drain(log) != NGX_OK)
    {
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
ngx_http_hoplite_rtc_send_udp(ngx_http_hoplite_rtc_session_t *session,
                              hoplite_rtc_poll_t *output, ngx_log_t *log)
{
    ngx_pool_t *pool;
    ngx_addr_t address;
    ssize_t sent;

    pool = ngx_create_pool(1024, log);
    if (pool == NULL) {
        return NGX_ERROR;
    }
    if (ngx_parse_addr(pool, &address, output->destination.data,
                       output->destination.len) != NGX_OK)
    {
        ngx_destroy_pool(pool);
        return NGX_ERROR;
    }
    sent = sendto(session->udp->fd, output->payload.data, output->payload.len,
                  0, address.sockaddr, address.socklen);
    ngx_destroy_pool(pool);
    return sent == (ssize_t) output->payload.len ? NGX_OK : NGX_ERROR;
}

static ngx_int_t
ngx_http_hoplite_rtc_pump(ngx_http_hoplite_rtc_session_t *session,
                          ngx_log_t *log)
{
    hoplite_rtc_poll_t output;
    ngx_int_t rc = NGX_OK;
    ngx_msec_t delay;

    while (hoplite_rtc_poll(session->engine, &output) == 0) {
        if (output.kind == 0) {
            if (session->timer.timer_set) {
                ngx_del_timer(&session->timer);
            }
            delay = output.timeout_millis > 3600000
                ? 3600000 : (ngx_msec_t) output.timeout_millis;
            ngx_add_timer(&session->timer, ngx_max(delay, 1));
        } else if (output.kind == 1) {
            if (ngx_http_hoplite_rtc_send_udp(session, &output, log) != NGX_OK) {
                rc = NGX_ERROR;
            }
        } else if (output.kind == 2) {
            if (session->receiver != NULL) {
                if (ngx_http_hoplite_rtc_deliver(session, &output.payload, log)
                    != NGX_OK)
                {
                    rc = NGX_ERROR;
                }
            } else if (session->received.data == NULL) {
                session->received = output.payload;
                output.payload.data = NULL;
                output.payload.len = 0;
            } else {
                ngx_log_error(NGX_LOG_WARN, log, 0,
                              "dropping RTC message: bounded receive slot is full");
            }
        }
        ngx_http_hoplite_rtc_buffer_free(&output.source);
        ngx_http_hoplite_rtc_buffer_free(&output.destination);
        ngx_http_hoplite_rtc_buffer_free(&output.payload);
        if (output.kind == 0 || rc != NGX_OK) {
            break;
        }
    }
    return rc;
}

static void
ngx_http_hoplite_rtc_timer_handler(ngx_event_t *event)
{
    ngx_http_hoplite_rtc_session_t *session = event->data;
    if (session == NULL
        || hoplite_rtc_handle_timeout(session->engine) != 0
        || ngx_http_hoplite_rtc_pump(session, event->log) != NGX_OK)
    {
        ngx_log_error(NGX_LOG_ERR, event->log, 0, "RTC timer drive failed");
    }
}

static void
ngx_http_hoplite_rtc_read_handler(ngx_event_t *event)
{
    ngx_connection_t *connection = event->data;
    ngx_http_hoplite_rtc_session_t *session = connection->data;
    struct sockaddr_storage source, local;
    socklen_t source_len, local_len;
    u_char packet[2048], source_text[NGX_SOCKADDR_STRLEN];
    u_char local_text[NGX_SOCKADDR_STRLEN];
    size_t source_text_len, local_text_len;
    ssize_t received;

    for ( ;; ) {
        source_len = sizeof(source);
        received = recvfrom(connection->fd, packet, sizeof(packet), 0,
                            (struct sockaddr *) &source, &source_len);
        if (received < 0) {
            if (ngx_socket_errno == NGX_EAGAIN) {
                return;
            }
            ngx_log_error(NGX_LOG_ERR, event->log, ngx_socket_errno,
                          "RTC recvfrom() failed");
            return;
        }
        local_len = sizeof(local);
        if (getsockname(connection->fd, (struct sockaddr *) &local,
                        &local_len) == -1)
        {
            return;
        }
        source_text_len = ngx_sock_ntop((struct sockaddr *) &source, source_len,
                                        source_text, sizeof(source_text), 1);
        local_text_len = ngx_sock_ntop((struct sockaddr *) &local, local_len,
                                       local_text, sizeof(local_text), 1);
        if (source_text_len == 0 || local_text_len == 0
            || hoplite_rtc_handle_udp(session->engine,
                                      source_text, source_text_len,
                                      local_text, local_text_len,
                                      packet, (size_t) received) != 0
            || ngx_http_hoplite_rtc_pump(session, event->log) != NGX_OK)
        {
            ngx_log_error(NGX_LOG_ERR, event->log, 0, "RTC UDP drive failed");
            return;
        }
    }
}

static void
ngx_http_hoplite_rtc_session_free(ngx_http_hoplite_rtc_session_t *session)
{
    if (session->timer.timer_set) {
        ngx_del_timer(&session->timer);
    }
    if (session->udp != NULL) {
        ngx_close_connection(session->udp);
    }
    ngx_http_hoplite_rtc_buffer_free(&session->received);
    hoplite_rtc_engine_free(session->engine);
    ngx_free(session);
}

static ngx_int_t
ngx_http_hoplite_rtc_bind(ngx_http_hoplite_rtc_session_t *session,
                          ngx_str_t *bind_address, ngx_pool_t *pool,
                          ngx_log_t *log)
{
    ngx_url_t url;
    ngx_socket_t fd;
    ngx_connection_t *connection;
    struct sockaddr_storage local;
    socklen_t local_len;
    u_char local_text[NGX_SOCKADDR_STRLEN];
    size_t local_text_len;
    ngx_int_t event_flags;

    ngx_memzero(&url, sizeof(url));
    url.url = *bind_address;
    url.no_resolve = 1;
    if (ngx_parse_url(pool, &url) != NGX_OK || url.naddrs != 1) {
        return NGX_ERROR;
    }
    fd = ngx_socket(url.addrs[0].sockaddr->sa_family, SOCK_DGRAM, 0);
    if (fd == (ngx_socket_t) -1) {
        return NGX_ERROR;
    }
    if (ngx_nonblocking(fd) == -1
        || bind(fd, url.addrs[0].sockaddr, url.addrs[0].socklen) == -1)
    {
        ngx_close_socket(fd);
        return NGX_ERROR;
    }
    connection = ngx_get_connection(fd, log);
    if (connection == NULL) {
        ngx_close_socket(fd);
        return NGX_ERROR;
    }
    connection->data = session;
    connection->read->handler = ngx_http_hoplite_rtc_read_handler;
    connection->read->log = log;
    connection->write->log = log;
    session->udp = connection;

    event_flags = (ngx_event_flags & NGX_USE_CLEAR_EVENT)
        ? NGX_CLEAR_EVENT : NGX_LEVEL_EVENT;
    if (ngx_add_event(connection->read, NGX_READ_EVENT, event_flags) != NGX_OK) {
        ngx_close_connection(connection);
        session->udp = NULL;
        return NGX_ERROR;
    }
    local_len = sizeof(local);
    if (getsockname(fd, (struct sockaddr *) &local, &local_len) == -1) {
        ngx_close_connection(connection);
        session->udp = NULL;
        return NGX_ERROR;
    }
    local_text_len = ngx_sock_ntop((struct sockaddr *) &local, local_len,
                                   local_text, sizeof(local_text), 1);
    if (local_text_len == 0
        || hoplite_rtc_add_local_udp_candidate(session->engine,
                                               local_text,
                                               local_text_len) != 0)
    {
        ngx_close_connection(connection);
        session->udp = NULL;
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
ngx_http_hoplite_rtc_create(const ngx_http_hoplite_host_call_t *call)
{
    hoplite_hta_value_t *options, *value;
    ngx_http_hoplite_rtc_session_t *session;
    ngx_str_t label, bind_address = ngx_string("127.0.0.1:0"), encoded;
    int64_t max_message = 65536;

    if (call->arguments == NULL
        || call->arguments->kind != HOPLITE_HTA_VECTOR
        || call->arguments->as.vector.count != 1
        || (options = call->arguments->as.vector.items[0]) == NULL
        || options->kind != HOPLITE_HTA_MAP
        || (value = hoplite_hta_map_get(options, "label")) == NULL
        || hoplite_hta_text(value, &label) != NGX_OK)
    {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.rtc/create expects [{:label string ...}]");
    }
    value = hoplite_hta_map_get(options, "max-message-bytes");
    if (value != NULL
        && (hoplite_hta_number(value, &max_message) != NGX_OK
            || max_message < 1 || max_message > 1048576))
    {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
                                       "hoplite.rtc max-message-bytes must be between 1 and 1048576");
    }
    value = hoplite_hta_map_get(options, "bind-address");
    if (value != NULL && hoplite_hta_text(value, &bind_address) != NGX_OK) {
        return ngx_http_hoplite_reject(call->call, call->pool,
                                       "hoplite.rtc bind-address must be a string");
    }
    session = ngx_alloc(sizeof(*session), call->log);
    if (session == NULL) {
        return NGX_ERROR;
    }
    ngx_memzero(session, sizeof(*session));
    session->engine = hoplite_rtc_engine_new(
        label.data, label.len, (size_t) max_message);
    if (session->engine == NULL) {
        ngx_free(session);
        return ngx_http_hoplite_reject(call->call, call->pool,
                                       "invalid hoplite.rtc configuration");
    }
    session->timer.handler = ngx_http_hoplite_rtc_timer_handler;
    session->timer.data = session;
    session->timer.log = call->log;
    if (ngx_http_hoplite_rtc_bind(session, &bind_address,
                                  call->pool, call->log) != NGX_OK)
    {
        ngx_http_hoplite_rtc_session_free(session);
        return ngx_http_hoplite_reject(call->call, call->pool,
                                       "could not bind hoplite.rtc UDP socket");
    }
    if (ngx_http_hoplite_rtc_next_id == UINT64_MAX) {
        ngx_http_hoplite_rtc_session_free(session);
        return NGX_ERROR;
    }
    session->id = ++ngx_http_hoplite_rtc_next_id;
    session->next = ngx_http_hoplite_rtc_sessions;
    ngx_http_hoplite_rtc_sessions = session;
    if (ngx_http_hoplite_rtc_pump(session, call->log) != NGX_OK
        || hoplite_hta_encode_number(call->pool, (int64_t) session->id,
                                  &encoded) != NGX_OK
        || hoplite_call_resolve(ngx_http_hoplite_runtime, call->call,
                                encoded.data, encoded.len) != 0)
    {
        ngx_http_hoplite_rtc_sessions = session->next;
        ngx_http_hoplite_rtc_session_free(session);
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
ngx_http_hoplite_rtc_invoke(const ngx_http_hoplite_host_call_t *call)
{
    ngx_http_hoplite_rtc_session_t *session, **link;
    hoplite_hta_value_t *value;
    hoplite_buffer_t buffer = {NULL, 0};
    ngx_str_t text;
    int rc;

    if (ngx_http_hoplite_operation(&call->operation, "create")
        || ngx_http_hoplite_operation(&call->operation, "connect"))
    {
        return ngx_http_hoplite_rtc_create(call);
    }
    if (ngx_http_hoplite_rtc_handle(call,
            (ngx_http_hoplite_operation(&call->operation, "create-offer")
             || ngx_http_hoplite_operation(&call->operation, "receive")
             || ngx_http_hoplite_operation(&call->operation, "close")) ? 1 : 2,
            &session) != NGX_OK)
    {
        return ngx_http_hoplite_reject(call->call, call->pool,
                                       "unknown hoplite.rtc session handle");
    }
    if (ngx_http_hoplite_operation(&call->operation, "create-offer")) {
        rc = hoplite_rtc_create_offer(session->engine, &buffer);
        if (rc != 0 || ngx_http_hoplite_rtc_pump(session, call->log) != NGX_OK) {
            ngx_http_hoplite_rtc_buffer_free(&buffer);
            return ngx_http_hoplite_reject(call->call, call->pool,
                                           "could not create RTC offer");
        }
        return ngx_http_hoplite_rtc_resolve_buffer(call, &buffer);
    }
    if (ngx_http_hoplite_operation(&call->operation, "receive")) {
        if (session->received.data != NULL) {
            return ngx_http_hoplite_rtc_resolve_buffer(call, &session->received);
        }
        if (session->receiver != NULL) {
            return ngx_http_hoplite_reject(call->call, call->pool,
                                           "RTC session already has a pending receive");
        }
        session->receiver = call->ctx;
        session->receive_call = call->call;
        return NGX_AGAIN;
    }
    value = call->arguments->as.vector.items[1];
    if (ngx_http_hoplite_operation(&call->operation, "accept-offer")) {
        if (hoplite_hta_text(value, &text) != NGX_OK
            || hoplite_rtc_accept_offer(session->engine, text.data, text.len,
                                        &buffer) != 0)
        {
            return ngx_http_hoplite_reject(call->call, call->pool,
                                           "invalid RTC offer");
        }
        if (ngx_http_hoplite_rtc_pump(session, call->log) != NGX_OK) {
            ngx_http_hoplite_rtc_buffer_free(&buffer);
            return NGX_ERROR;
        }
        return ngx_http_hoplite_rtc_resolve_buffer(call, &buffer);
    }
    if (ngx_http_hoplite_operation(&call->operation, "accept-answer")) {
        if (hoplite_hta_text(value, &text) != NGX_OK
            || hoplite_rtc_accept_answer(session->engine, text.data, text.len) != 0)
        {
            return ngx_http_hoplite_reject(call->call, call->pool,
                                           "invalid RTC answer");
        }
        return ngx_http_hoplite_rtc_pump(session, call->log) == NGX_OK
               && hoplite_call_resolve(ngx_http_hoplite_runtime,
                                       call->call, NULL, 0) == 0
            ? NGX_OK : NGX_ERROR;
    }
    if (ngx_http_hoplite_operation(&call->operation, "send")) {
        if (hoplite_hta_text(value, &text) != NGX_OK
            || hoplite_rtc_send(session->engine, text.data, text.len) < 0)
        {
            return ngx_http_hoplite_reject(call->call, call->pool,
                                           "RTC channel is not writable");
        }
        return ngx_http_hoplite_rtc_pump(session, call->log) == NGX_OK
               && hoplite_call_resolve(ngx_http_hoplite_runtime,
                                       call->call, NULL, 0) == 0
            ? NGX_OK : NGX_ERROR;
    }
    if (ngx_http_hoplite_operation(&call->operation, "close")) {
        for (link = &ngx_http_hoplite_rtc_sessions; *link != NULL;
             link = &(*link)->next)
        {
            if (*link == session) {
                *link = session->next;
                if (session->receiver != NULL) {
                    ngx_http_hoplite_ctx_t *receiver = session->receiver;
                    uint64_t receive_call = session->receive_call;
                    receiver->provider = NULL;
                    session->receiver = NULL;
                    session->receive_call = 0;
                    if (!receiver->done
                        && hoplite_call_resolve(ngx_http_hoplite_runtime,
                                                receive_call, NULL, 0) == 0)
                    {
                        (void) ngx_http_hoplite_drain(call->log);
                    }
                }
                ngx_http_hoplite_rtc_session_free(session);
                break;
            }
        }
        return hoplite_call_resolve(ngx_http_hoplite_runtime,
                                    call->call, NULL, 0) == 0 ? NGX_OK : NGX_ERROR;
    }
    return ngx_http_hoplite_reject(call->call, call->pool,
                                   "unsupported hoplite.rtc operation");
}

static void
ngx_http_hoplite_rtc_cancel(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_rtc_session_t *session;
    for (session = ngx_http_hoplite_rtc_sessions;
         session != NULL; session = session->next)
    {
        if (session->receiver == ctx) {
            session->receiver = NULL;
            session->receive_call = 0;
        }
    }
}

static void
ngx_http_hoplite_rtc_clear(void)
{
    ngx_http_hoplite_rtc_session_t *session;
    while (ngx_http_hoplite_rtc_sessions != NULL) {
        session = ngx_http_hoplite_rtc_sessions;
        ngx_http_hoplite_rtc_sessions = session->next;
        ngx_http_hoplite_rtc_session_free(session);
    }
    ngx_http_hoplite_rtc_next_id = 0;
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
        ngx_log_error(NGX_LOG_ERR, log, 0,
                      "unsupported Hoplite host service: service=%V operation=%V",
                      &service, &operation);
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
    ngx_log_error(NGX_LOG_NOTICE, log, 0,
                  "invoking native Hoplite provider: service=%V operation=%V",
                  &service, &operation);
    native_rc = native_provider->invoke(&native_call);
    ngx_log_error(NGX_LOG_NOTICE, log, 0,
                  "native Hoplite provider returned: service=%V operation=%V status=%d completed=%d",
                  &service, &operation, native_rc, completion->completed);
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
    ngx_log_error(NGX_LOG_ERR, log, 0,
                  "native Hoplite provider failed: service=%V operation=%V status=%d",
                  &service, &operation, native_rc);
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
        ngx_http_hoplite_request_failure(event->log, "host-suspension");
        ngx_http_hoplite_send(ctx, NGX_HTTP_INTERNAL_SERVER_ERROR, &body, NULL);
    }
}

static void
ngx_http_hoplite_timeout_handler(ngx_event_t *event)
{
    ngx_http_hoplite_ctx_t *ctx = event->data;
    ngx_str_t body = ngx_string("Hoplite request timed out\n");

    if (ctx == NULL || ctx->done) {
        return;
    }
    ngx_http_hoplite_request_failure(event->log, "timeout");
    ngx_http_hoplite_send(ctx, NGX_HTTP_GATEWAY_TIME_OUT, &body, NULL);
}

static void
ngx_http_hoplite_cleanup(void *data)
{
    ngx_http_hoplite_ctx_t *ctx = data;

    if (ctx->timeout.timer_set) {
        ngx_del_timer(&ctx->timeout);
    }
    if (!ctx->done && ctx->request != NULL) {
        ngx_http_hoplite_request_failure(
            ctx->request->connection->log, "disconnect");
    }

    ngx_http_hoplite_response_source_release(ctx);
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

    if (!ctx->done && ngx_http_hoplite_runtime != NULL && ctx->work != 0) {
        if (hoplite_work_cancel(ngx_http_hoplite_runtime, ctx->work) != 0) {
            ngx_http_hoplite_request_failure(
                ctx->request->connection->log, "cancellation");
        }
        if (hoplite_work_close(ngx_http_hoplite_runtime, ctx->work) != 0) {
            ngx_http_hoplite_request_failure(
                ctx->request->connection->log, "cleanup");
        }
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
            ngx_http_hoplite_request_failure(
                request->connection->log, "routing");
            ngx_log_error(NGX_LOG_ERR, request->connection->log, 0,
                          "Hoplite app invocation failed: app=%ui rc=%i",
                          conf->app, rc);
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (outcome.kind == 1) {
            /* send_native borrows response slices until request cleanup. */
            ctx->response = outcome.id;
            return ngx_http_hoplite_send_native(ctx);
        }
        if (outcome.kind != 2 || outcome.id == 0) {
            ngx_http_hoplite_request_failure(
                request->connection->log, "unsupported-yield");
            ngx_log_error(NGX_LOG_ERR, request->connection->log, 0,
                          "Hoplite app returned an invalid outcome: kind=%ui id=%uL",
                          outcome.kind, outcome.id);
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
            ngx_http_hoplite_request_failure(
                request->connection->log, "routing");
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        if (outcome.kind == 1) {
            /* send_native borrows response slices until request cleanup. */
            ctx->response = outcome.id;
            return ngx_http_hoplite_send_native(ctx);
        }
        if (outcome.kind != 2 || outcome.id == 0) {
            ngx_http_hoplite_request_failure(
                request->connection->log, "unsupported-yield");
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        ctx->work = outcome.id;
    }

    ngx_queue_insert_tail(&ngx_http_hoplite_requests, &ctx->queue);
    ctx->queued = 1;
    request->main->count++;
    ctx->timeout.handler = ngx_http_hoplite_timeout_handler;
    ctx->timeout.data = ctx;
    ctx->timeout.log = request->connection->log;
    ctx->timeout.cancelable = 1;
    ngx_add_timer(&ctx->timeout, conf->request_timeout);
    if (ngx_http_hoplite_drain(request->connection->log) != NGX_OK) {
        ngx_log_error(NGX_LOG_ERR, request->connection->log, 0,
                      "Hoplite work drain failed: work=%uL", ctx->work);
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
        ngx_http_hoplite_request_failure(
            request->connection->log, "body-limit");
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
ngx_http_hoplite_startup_diagnostic(void *context,
                                    const uint8_t *diagnostic,
                                    size_t diagnostic_len)
{
    ngx_cycle_t *cycle = context;
    ngx_str_t value;
    value.data = (u_char *) diagnostic;
    value.len = diagnostic_len;
    ngx_log_error(NGX_LOG_NOTICE, cycle->log, 0,
                  "hoplite startup: %V", &value);
    return NGX_OK;
}

static void
ngx_http_hoplite_startup_callback(void *context,
                                  const uint8_t *diagnostic,
                                  size_t diagnostic_len)
{
    (void) ngx_http_hoplite_startup_diagnostic(
        context, diagnostic, diagnostic_len);
}

static void
ngx_http_hoplite_startup_outer(ngx_cycle_t *cycle,
                               ngx_uint_t sequence,
                               const char *stage,
                               const char *status,
                               const char *failure_class)
{
    if (failure_class == NULL) {
        ngx_log_error(
            NGX_LOG_NOTICE, cycle->log, 0,
            "hoplite startup: {\"format\":\"hoplite.startup-diagnostic/0-alpha\",\"sequence\":%ui,\"stage\":\"%s\",\"status\":\"%s\"}",
            sequence, stage, status);
    } else {
        ngx_log_error(
            NGX_LOG_NOTICE, cycle->log, 0,
            "hoplite startup: {\"format\":\"hoplite.startup-diagnostic/0-alpha\",\"sequence\":%ui,\"stage\":\"%s\",\"status\":\"%s\",\"class\":\"%s\"}",
            sequence, stage, status, failure_class);
    }
}

static ngx_int_t
ngx_http_hoplite_bootstrap(ngx_cycle_t *cycle,
                           const ngx_str_t *bundle_path,
                           const ngx_str_t *manifest_path)
{
    if (hoplite_bootstrap_application_files_v2(
            ngx_http_hoplite_runtime,
            bundle_path->data,
            bundle_path->len,
            manifest_path->data,
            manifest_path->len,
            ngx_http_hoplite_startup_callback,
            cycle) != 0)
    {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite HAB0 application bootstrap loading failed");
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
ngx_http_hoplite_init_process(ngx_cycle_t *cycle)
{
    ngx_http_hoplite_main_conf_t *conf;

    ngx_queue_init(&ngx_http_hoplite_requests);
    ngx_http_hoplite_queue_ready = 1;
    hoplite_host_registry_init(&ngx_http_hoplite_providers);
    if (ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_nginx_provider) != NGX_OK
        || ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_rtc_provider) != NGX_OK)
    {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite native host providers could not be registered");
        return NGX_ERROR;
    }
    ngx_http_hoplite_runtime = hoplite_runtime_new();
    if (ngx_http_hoplite_runtime == NULL || hoplite_abi_version() < 4) {
        ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                      "hoplite runtime could not be initialized");
        return NGX_ERROR;
    }

    conf = ngx_http_cycle_get_module_main_conf(cycle, ngx_http_hoplite_module);
    if (conf != NULL
        && (conf->bootstrap.len != 0 || conf->manifest.len != 0))
    {
        if (conf->bootstrap.len == 0 || conf->manifest.len == 0) {
            ngx_http_hoplite_startup_outer(
                cycle, 1, "configuration", "failed",
                "configuration-incomplete");
            ngx_log_error(NGX_LOG_EMERG, cycle->log, 0,
                          "hoplite_bootstrap and hoplite_manifest must be configured together");
            return NGX_ERROR;
        }
        ngx_http_hoplite_startup_outer(
            cycle, 1, "configuration", "ok", NULL);
        if (ngx_http_hoplite_bootstrap(cycle,
                                       &conf->bootstrap,
                                       &conf->manifest) != NGX_OK)
        {
            return NGX_ERROR;
        }
    } else {
        ngx_http_hoplite_startup_outer(
            cycle, 1, "configuration", "ok", NULL);
    }
    ngx_http_hoplite_startup_outer(cycle, 5, "readiness", "ok", NULL);
    return NGX_OK;
}

static void
ngx_http_hoplite_exit_process(ngx_cycle_t *cycle)
{
    (void) cycle;
    ngx_http_hoplite_rtc_clear();
    if (ngx_http_hoplite_runtime != NULL) {
        hoplite_runtime_free(ngx_http_hoplite_runtime);
        ngx_http_hoplite_runtime = NULL;
    }
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
        conf->request_timeout = NGX_CONF_UNSET_MSEC;
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
    ngx_conf_merge_msec_value(conf->request_timeout,
                              previous->request_timeout, 30000);
    if (conf->request_body_chunk == 0
        || conf->request_body_max == 0
        || conf->request_body_chunk > conf->request_body_max)
    {
        return "hoplite request body limits must be positive and chunk <= max";
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
