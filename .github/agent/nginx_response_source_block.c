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
        || value->as.map.count != 4)
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

        } else if (ngx_http_hoplite_response_source_name(
                       pair->key, "source-handle"))
        {
            if ((seen & 2u) != 0
                || hoplite_hta_number(pair->value, &number) != NGX_OK
                || number < 0)
            {
                return NGX_ERROR;
            }
            descriptor->source_handle = (uint64_t) number;
            seen |= 2u;

        } else if (ngx_http_hoplite_response_source_name(
                       pair->key, "offset"))
        {
            if ((seen & 4u) != 0
                || hoplite_hta_number(pair->value, &number) != NGX_OK
                || number < 0)
            {
                return NGX_ERROR;
            }
            descriptor->offset = (uint64_t) number;
            seen |= 4u;

        } else if (ngx_http_hoplite_response_source_name(
                       pair->key, "length"))
        {
            if ((seen & 8u) != 0
                || hoplite_hta_number(pair->value, &number) != NGX_OK
                || number < 0)
            {
                return NGX_ERROR;
            }
            descriptor->length = (uint64_t) number;
            seen |= 8u;

        } else {
            return NGX_ERROR;
        }
    }

    return seen == 15u
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
    return hoplite_blob_host_provider_response_read_v1(
               request_context,
               work,
               source_handle,
               output,
               capacity,
               returned) == HOPLITE_BLOB_HOST_PROVIDER_OK
        ? HOPLITE_RESPONSE_SOURCE_OK
        : HOPLITE_RESPONSE_SOURCE_ERROR;
}

static int32_t
ngx_http_hoplite_response_source_close(void *request_context,
                                       uint64_t work,
                                       uint64_t source_handle)
{
    return hoplite_blob_host_provider_response_close_v1(
               request_context,
               work,
               source_handle) == HOPLITE_BLOB_HOST_PROVIDER_OK
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

    rc = hoplite_response_source_next_v1(
        &stream->source,
        stream->storage,
        stream->capacity,
        &returned,
        &last);
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

        if (ngx_http_hoplite_response_source_fill(ctx) != NGX_OK) {
            ngx_http_hoplite_response_source_abort(ctx, NGX_ERROR);
            return NGX_ERROR;
        }
    }
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
    ngx_int_t rc;

    request = ctx->request;
    stream = &ctx->response_source;
    if (ctx->work == 0
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
