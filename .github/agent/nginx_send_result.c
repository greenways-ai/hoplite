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
                "Hoplite response source must be an exact hara.response-source/1 map\n");
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
