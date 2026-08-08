from pathlib import Path

MODULE = Path("core/nginx/ngx_http_hoplite_module.c")
APP = Path("core/examples/app.hal")
BODY_SMOKE = Path("packaging/scripts/smoke-request-body-v3.sh")
SOURCE_SMOKE = Path("packaging/scripts/smoke-response-source.sh")
SELF = Path(__file__)


def replace_once(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise SystemExit(f"expected one {label}, found {source.count(old)}")
    return source.replace(old, new)


source = MODULE.read_text()

source = replace_once(
    source,
    """    size_t request_body_max;
    size_t request_body_chunk;
} ngx_http_hoplite_loc_conf_t;
""",
    """    size_t request_body_max;
    size_t request_body_chunk;
    size_t response_body_chunk;
} ngx_http_hoplite_loc_conf_t;
""",
    "location response chunk field",
)

source = replace_once(
    source,
    """typedef struct {
    ngx_http_request_t *request;
    ngx_chain_t *chain;
    off_t offset;
    ngx_flag_t closed;
} ngx_http_hoplite_body_t;

typedef struct ngx_http_hoplite_ctx_s ngx_http_hoplite_ctx_t;
""",
    """typedef struct {
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
""",
    "response source state types",
)

source = replace_once(
    source,
    """    const hoplite_host_provider_v1_t *native_provider;
    ngx_http_hoplite_native_completion_t *native_completion;
    ngx_http_hoplite_body_t body;
};
""",
    """    const hoplite_host_provider_v1_t *native_provider;
    ngx_http_hoplite_native_completion_t *native_completion;
    ngx_http_hoplite_body_t body;
    ngx_http_hoplite_source_t source;
};
""",
    "request response source field",
)

source = replace_once(
    source,
    """static void ngx_http_hoplite_cleanup(void *data);
static void ngx_http_hoplite_sleep_handler(ngx_event_t *event);
static ngx_int_t ngx_http_hoplite_drain(ngx_log_t *log);
""",
    """static void ngx_http_hoplite_cleanup(void *data);
static void ngx_http_hoplite_sleep_handler(ngx_event_t *event);
static ngx_int_t ngx_http_hoplite_drain(ngx_log_t *log);
static void ngx_http_hoplite_source_close(ngx_http_hoplite_ctx_t *ctx);
static void ngx_http_hoplite_source_write_handler(ngx_http_request_t *request);
""",
    "response source forward declarations",
)

source = replace_once(
    source,
    """    {
        ngx_string("hoplite_request_body_chunk"),
        NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1,
        ngx_conf_set_size_slot,
        NGX_HTTP_LOC_CONF_OFFSET,
        offsetof(ngx_http_hoplite_loc_conf_t, request_body_chunk),
        NULL
    },
    ngx_null_command
};
""",
    """    {
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
""",
    "response chunk directive",
)

finish_marker = """static void
ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)
{
"""
source_close = """static void
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
"""
source = replace_once(source, finish_marker, source_close, "source close insertion")
source = replace_once(
    source,
    """ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)
{
    if (ctx->done) {
""",
    """ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)
{
    ngx_http_hoplite_source_close(ctx);
    if (ctx->done) {
""",
    "finish source close",
)

send_native_marker = """static ngx_int_t
ngx_http_hoplite_send_native(ngx_http_hoplite_ctx_t *ctx)
"""
source_functions = r'''static ngx_flag_t
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
        if (source->final_read) {
            ngx_http_hoplite_source_complete(ctx, rc);
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
'''
source = replace_once(source, send_native_marker, source_functions, "response source engine")

send_result_start = source.index(
    "static void\nngx_http_hoplite_send_result(ngx_http_hoplite_ctx_t *ctx,"
)
send_result_end = source.index(
    "static void\nngx_http_hoplite_send_error(", send_result_start
)
new_send_result = r'''static void
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

'''
source = source[:send_result_start] + new_send_result + source[send_result_end:]

source = replace_once(
    source,
    """    ctx->provider = NULL;
    ctx->native_provider = NULL;

    if (ctx->work != 0) {
""",
    """    ctx->provider = NULL;
    ctx->native_provider = NULL;
    ngx_http_hoplite_source_close(ctx);

    if (ctx->work != 0) {
""",
    "cleanup source close",
)

source = replace_once(
    source,
    """        conf->request_body_max = NGX_CONF_UNSET_SIZE;
        conf->request_body_chunk = NGX_CONF_UNSET_SIZE;
""",
    """        conf->request_body_max = NGX_CONF_UNSET_SIZE;
        conf->request_body_chunk = NGX_CONF_UNSET_SIZE;
        conf->response_body_chunk = NGX_CONF_UNSET_SIZE;
""",
    "response chunk initialization",
)

source = replace_once(
    source,
    """    ngx_conf_merge_size_value(conf->request_body_chunk,
                              previous->request_body_chunk, 64 * 1024);
    if (conf->request_body_chunk == 0
""",
    """    ngx_conf_merge_size_value(conf->request_body_chunk,
                              previous->request_body_chunk, 64 * 1024);
    ngx_conf_merge_size_value(conf->response_body_chunk,
                              previous->response_body_chunk, 64 * 1024);
    if (conf->request_body_chunk == 0
""",
    "response chunk merge",
)

source = replace_once(
    source,
    """        return "hoplite request body limits must be positive and chunk <= max";
    }
    if (conf->request_body
""",
    """        return "hoplite request body limits must be positive and chunk <= max";
    }
    if (conf->response_body_chunk == 0) {
        return "hoplite response body chunk must be positive";
    }
    if (conf->request_body
""",
    "response chunk validation",
)

MODULE.write_text(source)

app = APP.read_text()
app = replace_once(
    app,
    """;; This example is also the production-image conformance surface for the
;; request-body V3 adapter and its bounded native body-handle route.
""",
    """;; This example is also the production-image conformance surface for the
;; request-body V3 adapter, the generic hara.blob provider, and bounded
;; source-backed HTTP responses.

(def blob-size 524288)
(def blob-digest
  "sha256:afc2d0f78b79cea2f147edb19e71a881ef17a184d102e52cf6a7fe1c2733ab12")
(def blob-staging-key "hoplite-response-source-smoke")
""",
    "example blob constants",
)

app_functions = r'''
(defn ^:async blob-upload
  [request]
  (let [source (:body-handle request)]
    (std.foundation.coroutine/await
      (std.native.Host/call
        "hara.blob"
        "staging/open"
        [{:protocol "hara.blob-request/1"
          :operation "staging/open"
          :staging-key blob-staging-key
          :expected-digest blob-digest
          :expected-size blob-size
          :media-type "text/plain; charset=utf-8"}]))
    (std.foundation.coroutine/await
      (std.native.Host/call
        "hara.blob"
        "staging/append-from-source"
        [{:protocol "hara.blob-request/1"
          :operation "staging/append-from-source"
          :staging-key blob-staging-key
          :source-handle source
          :offset 0
          :length blob-size}]))
    (std.foundation.coroutine/await
      (std.native.Host/call
        "hara.blob"
        "staging/verify-commit"
        [{:protocol "hara.blob-request/1"
          :operation "staging/verify-commit"
          :staging-key blob-staging-key
          :expected-digest blob-digest
          :expected-size blob-size}]))
    {:status 201
     :headers {"content-type" "text/plain; charset=utf-8"}
     :body "Stored response source fixture\n"}))

(defn source-response
  [status opened headers]
  {:status status
   :headers headers
   :body {:protocol "hoplite.response-source/1"
          :source (:source-handle opened)
          :offset (:offset opened)
          :length (:length opened)}})

(defn ^:async blob-source
  [request]
  (let [opened
        (std.foundation.coroutine/await
          (std.native.Host/call
            "hara.blob"
            "object/open-source"
            [{:protocol "hara.blob-request/1"
              :operation "object/open-source"
              :digest blob-digest
              :offset 0
              :length blob-size}]))]
    (source-response
      200
      opened
      {"content-type" "text/plain; charset=utf-8"
       "etag" blob-digest
       "accept-ranges" "bytes"})))

(defn ^:async blob-source-range
  [request]
  (let [opened
        (std.foundation.coroutine/await
          (std.native.Host/call
            "hara.blob"
            "object/open-source"
            [{:protocol "hara.blob-request/1"
              :operation "object/open-source"
              :digest blob-digest
              :offset 17
              :length 4096}]))]
    (source-response
      206
      opened
      {"content-type" "text/plain; charset=utf-8"
       "etag" blob-digest
       "content-range" "bytes 17-4112/524288"
       "accept-ranges" "bytes"})))

(defn invalid-source-plan
  [request]
  {:status 200
   :body {:protocol "hoplite.response-source/1"
          :source 1
          :offset 0
          :length 1
          :unexpected true}})

(defn stale-source-plan
  [request]
  {:status 200
   :body {:protocol "hoplite.response-source/1"
          :source 900719925474099
          :offset 0
          :length 1}})

'''
app = replace_once(app, "\n(def app\n", app_functions + "(def app\n", "blob example functions")
app = replace_once(
    app,
    """     :request/body {:max-bytes 32
                    :max-chunk-bytes 8}
""",
    """     :request/body {:max-bytes 524288
                    :max-chunk-bytes 8192}
""",
    "example request body limits",
)
app = replace_once(
    app,
    """      ["/body-handle"
       {:post {:name "body-handle"
               :summary "Projects an opaque native body handle"
               :handler #'body-handle}}]]}))
""",
    """      ["/body-handle"
       {:post {:name "body-handle"
               :summary "Projects an opaque native body handle"
               :handler #'body-handle}}]
      ["/blob/upload"
       {:post {:name "blob-upload"
               :summary "Stores the deterministic response-source fixture"
               :handler #'blob-upload}}]
      ["/blob/source"
       {:get {:name "blob-source"
              :summary "Streams a complete immutable source"
              :handler #'blob-source}
        :head {:name "blob-source-head"
               :summary "Sends source headers without reading body bytes"
               :handler #'blob-source}}]
      ["/blob/source-range"
       {:get {:name "blob-source-range"
              :summary "Streams one exact immutable range"
              :handler #'blob-source-range}}]
      ["/blob/invalid-plan"
       {:get {:name "invalid-source-plan"
              :handler #'invalid-source-plan}}]
      ["/blob/stale-source"
       {:get {:name "stale-source-plan"
              :handler #'stale-source-plan}}]]}))
""",
    "blob example routes",
)
APP.write_text(app)

body_smoke = BODY_SMOKE.read_text()
body_smoke = replace_once(
    body_smoke,
    """body_file="$(mktemp)"
headers_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file"
}
""",
    """body_file="$(mktemp)"
headers_file="$(mktemp)"
large_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file" "$large_file"
}
""",
    "request body smoke large fixture",
)
body_smoke = replace_once(
    body_smoke,
    """large="$(printf '%040d' 0)"
status="$(request POST "$base/body-handle" --data-binary "$large")"
""",
    """python3 - "$large_file" <<'PY'
from pathlib import Path
import sys
Path(sys.argv[1]).write_bytes(b"x" * 524289)
PY
status="$(request POST "$base/body-handle" --data-binary @"$large_file")"
""",
    "request body oversized fixture",
)
BODY_SMOKE.write_text(body_smoke)

SOURCE_SMOKE.write_text(r'''#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-response-source-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
fixture_file="$(mktemp)"
body_file="$(mktemp)"
headers_file="$(mktemp)"
range_file="$(mktemp)"
slow_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$fixture_file" "$body_file" "$headers_file" "$range_file" "$slow_file"
}
trap cleanup EXIT INT TERM

diagnose() {
  echo '--- docker ps ---' >&2
  docker ps -a --filter "name=^/${container}$" >&2 || true
  echo '--- container state ---' >&2
  docker inspect "$container" --format '{{json .State}}' >&2 || true
  echo '--- container logs ---' >&2
  docker logs "$container" >&2 || true
  echo '--- generated nginx configuration ---' >&2
  docker exec "$container" sh -c 'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
}

request() {
  local method="$1"
  local url="$2"
  shift 2
  curl --silent --show-error \
    --max-time 30 \
    --request "$method" \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$@" \
    "$url"
}

header_value() {
  local name="$1"
  awk -v expected="$name" '
    BEGIN { IGNORECASE = 1 }
    index($0, expected ":") == 1 {
      sub(/^[^:]*:[[:space:]]*/, "")
      sub(/\r$/, "")
      value = $0
    }
    END { print value }
  ' "$headers_file"
}

python3 - "$fixture_file" <<'PY'
from pathlib import Path
import sys
pattern = b"greenways-hoplite-response-source\n"
size = 524288
Path(sys.argv[1]).write_bytes((pattern * ((size + len(pattern) - 1) // len(pattern)))[:size])
PY

docker run --detach --name "$container" -p 127.0.0.1::8080 "$image" >/dev/null

port=''
for _ in $(seq 1 50); do
  port="$(docker port "$container" 8080/tcp 2>/dev/null \
    | head -n 1 | awk -F: '{print $NF}' || true)"
  if [[ -n "$port" ]]; then
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]]; then
    diagnose
    exit 1
  fi
  sleep .1
done
if [[ -z "$port" ]]; then
  diagnose
  exit 1
fi

base="http://127.0.0.1:${port}"
ready=false
for _ in $(seq 1 60); do
  status="$(request GET "$base/hello" || true)"
  if [[ "$status" == 200 ]] && [[ "$(cat "$body_file")" == 'Hello from Hoplite' ]]; then
    ready=true
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  diagnose
  exit 1
fi

status="$(request POST "$base/blob/upload" --data-binary @"$fixture_file")"
if [[ "$status" != 201 ]] || [[ "$(cat "$body_file")" != 'Stored response source fixture' ]]; then
  echo "blob upload failed: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi

status="$(request GET "$base/blob/source")"
if [[ "$status" != 200 ]] || ! cmp -s "$fixture_file" "$body_file"; then
  echo "complete response source failed: status=$status" >&2
  diagnose
  exit 1
fi
if [[ "$(header_value Content-Length)" != 524288 ]]; then
  echo "complete response source has wrong content length" >&2
  diagnose
  exit 1
fi

status="$(curl --silent --show-error --max-time 30 \
  --head \
  --dump-header "$headers_file" \
  --output "$body_file" \
  --write-out '%{http_code}' \
  "$base/blob/source")"
if [[ "$status" != 200 ]] || [[ "$(header_value Content-Length)" != 524288 ]] \
  || [[ -s "$body_file" ]]; then
  echo "HEAD response source failed: status=$status length=$(header_value Content-Length)" >&2
  diagnose
  exit 1
fi

status="$(request GET "$base/blob/source-range")"
dd if="$fixture_file" of="$range_file" bs=1 skip=17 count=4096 status=none
if [[ "$status" != 206 ]] || ! cmp -s "$range_file" "$body_file"; then
  echo "range response source failed: status=$status" >&2
  diagnose
  exit 1
fi
if [[ "$(header_value Content-Range)" != 'bytes 17-4112/524288' ]] \
  || [[ "$(header_value Content-Length)" != 4096 ]]; then
  echo "range response source headers are invalid" >&2
  diagnose
  exit 1
fi

status="$(curl --silent --show-error --max-time 30 \
  --limit-rate 64k \
  --dump-header "$headers_file" \
  --output "$slow_file" \
  --write-out '%{http_code}' \
  "$base/blob/source")"
if [[ "$status" != 200 ]] || ! cmp -s "$fixture_file" "$slow_file"; then
  echo "slow response source resumption failed: status=$status" >&2
  diagnose
  exit 1
fi

for route in invalid-plan stale-source; do
  status="$(request GET "$base/blob/$route")"
  if [[ "$status" != 500 ]]; then
    echo "response source rejection failed for $route: status=$status" >&2
    diagnose
    exit 1
  fi
done

status="$(request GET "$base/hello")"
if [[ "$status" != 200 ]] || [[ "$(cat "$body_file")" != 'Hello from Hoplite' ]]; then
  echo 'response source failures damaged the worker queue' >&2
  diagnose
  exit 1
fi

printf 'Validated source-backed HTTP responses through %s.\n' "$image"
''')

SELF.unlink()
