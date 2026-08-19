#include "hoplite_cosocket.h"

#include <sys/socket.h>

/*
 * Keep the established TCP implementation in one translation unit while this
 * compatibility slice extends provider dispatch with send-direction shutdown.
 * The included core keeps its worker lifecycle and request-owned state; only
 * its provider entry points are renamed so the wrapper can add one operation.
 */
#define hoplite_cosocket_provider hoplite_cosocket_core_provider
#define hoplite_cosocket_invoke hoplite_cosocket_core_invoke
#define hoplite_cosocket_register hoplite_cosocket_core_register
#include "hoplite_cosocket.c"
#undef hoplite_cosocket_register
#undef hoplite_cosocket_invoke
#undef hoplite_cosocket_provider

static ngx_flag_t
hoplite_cosocket_send_is_shutdown(hoplite_cosocket_t *socket)
{
    hoplite_cosocket_reader_t *state;

    if (socket == NULL) {
        return 0;
    }
    for (state = socket->readers; state != NULL; state = state->next) {
        if (state->id == 0 && state->pattern == NULL) {
            return 1;
        }
    }
    return 0;
}

static ngx_int_t
hoplite_cosocket_shutdown(const hoplite_host_call_v1_t *call,
                          const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    hoplite_cosocket_reader_t *state;
    ngx_str_t direction;
    ngx_err_t error;
    const char *message;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/shutdown expects [socket direction]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    if (hoplite_hta_text(arguments->as.vector.items[1], &direction) != NGX_OK
        || direction.len != sizeof("send") - 1
        || ngx_strncmp(direction.data, "send", sizeof("send") - 1) != 0)
    {
        return hoplite_cosocket_reject(
            call, "shutdown direction must be send");
    }
    if (socket->closed || !socket->connected
        || socket->connection == NULL
        || hoplite_cosocket_send_is_shutdown(socket))
    {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (socket->pending == HOPLITE_COSOCKET_PENDING_SEND) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "socket busy writing");
    }

    state = ngx_pcalloc(socket->pool, sizeof(*state));
    if (state == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (shutdown(socket->connection->fd, SHUT_WR) == -1) {
        error = ngx_socket_errno;
        message = hoplite_cosocket_error_text(error, "shutdown failed");
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, message);
    }

    /* Reader handles start at one, so zero is an internal write-half marker. */
    state->id = 0;
    state->pattern = NULL;
    state->next = socket->readers;
    socket->readers = state;
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static int32_t
hoplite_cosocket_invoke(const hoplite_host_call_v1_t *call)
{
    ngx_pool_t *pool;
    hoplite_hta_value_t *arguments = NULL;
    hoplite_cosocket_t *socket;
    ngx_int_t rc;

    if (call == NULL
        || call->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION
        || call->request_context == NULL
        || call->work == 0 || call->call == 0
        || call->operation.data == NULL || call->operation.len == 0
        || call->completer.succeed == NULL || call->completer.fail == NULL)
    {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (!hoplite_cosocket_operation(call, "shutdown")
        && !hoplite_cosocket_operation(call, "send"))
    {
        return hoplite_cosocket_core_invoke(call);
    }

    pool = ngx_create_pool(4096, hoplite_cosocket_log);
    if (pool == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (hoplite_cosocket_decode_arguments(call, pool, &arguments) != NGX_OK) {
        ngx_destroy_pool(pool);
        return hoplite_cosocket_reject(
            call, "hoplite.socket arguments must be one HTA vector");
    }

    if (hoplite_cosocket_operation(call, "shutdown")) {
        rc = hoplite_cosocket_shutdown(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_OK && hoplite_cosocket_send_is_shutdown(socket)) {
        rc = hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }

    ngx_destroy_pool(pool);
    return hoplite_cosocket_core_invoke(call);
}

static const hoplite_host_provider_v1_t hoplite_cosocket_provider = {
    HOPLITE_HOST_PROVIDER_ABI_VERSION,
    {(const uint8_t *) "hoplite.socket", sizeof("hoplite.socket") - 1},
    hoplite_cosocket_invoke,
    hoplite_cosocket_cancel,
    0,
    NULL,
    NULL,
    NULL
};

ngx_int_t
hoplite_cosocket_register(ngx_cycle_t *cycle)
{
    int32_t rc;

    if (cycle == NULL || cycle->log == NULL) {
        return NGX_ERROR;
    }
    hoplite_cosocket_log = cycle->log;
    hoplite_cosockets = NULL;
    hoplite_cosocket_next_id = 0;
    hoplite_cosocket_next_reader_id = 0;
    rc = hoplite_host_provider_register_v1(&hoplite_cosocket_provider);
    return rc == HOPLITE_HOST_PROVIDER_REGISTER_OK ? NGX_OK : NGX_ERROR;
}
