#include <ngx_config.h>

#define HOPLITE_COSOCKET_BASE_IMPLEMENTATION 1

#include "hoplite_host_provider.h"
#include "hoplite_cosocket.h"

static int32_t hoplite_cosocket_directional_invoke(
    const hoplite_host_call_v1_t *call);
static void hoplite_cosocket_directional_cancel(void *request_context);

static const hoplite_host_provider_v1_t hoplite_cosocket_directional_provider = {
    HOPLITE_HOST_PROVIDER_ABI_VERSION,
    {(const uint8_t *) "hoplite.socket", sizeof("hoplite.socket") - 1},
    hoplite_cosocket_directional_invoke,
    hoplite_cosocket_directional_cancel,
    0,
    NULL,
    NULL,
    NULL
};

static int32_t
hoplite_cosocket_directional_register_provider(
    const hoplite_host_provider_v1_t *provider)
{
    (void) provider;
    return hoplite_host_provider_register_v1(
        &hoplite_cosocket_directional_provider);
}

/*
 * Reuse the established resolver, Unix-domain, keepalive-pool, and backlog
 * implementation as the lifecycle/read substrate.  Only provider registration
 * is intercepted: the public provider below gives writes their own operation
 * state while the included implementation retains connect and receive state.
 */
#define hoplite_host_provider_register_v1 \
    hoplite_cosocket_directional_register_provider
#include "hoplite_cosocket_shutdown.c"
#undef hoplite_host_provider_register_v1

typedef struct hoplite_cosocket_directional_write_s
    hoplite_cosocket_directional_write_t;

struct hoplite_cosocket_directional_write_s {
    hoplite_cosocket_t *socket;
    ngx_connection_t *connection;
    void *owner;
    uint64_t work;
    uint64_t call;
    hoplite_host_completer_v1_t completer;
    u_char *data;
    size_t len;
    size_t offset;
    ngx_flag_t active;
    ngx_flag_t linked;
    ngx_flag_t released;
    hoplite_cosocket_directional_write_t *next;
};

static hoplite_cosocket_directional_write_t
    *hoplite_cosocket_directional_writes;

static void hoplite_cosocket_directional_read_handler(ngx_event_t *event);
static void hoplite_cosocket_directional_write_handler(ngx_event_t *event);

static void
hoplite_cosocket_directional_unlink(
    hoplite_cosocket_directional_write_t *state)
{
    hoplite_cosocket_directional_write_t **link;

    if (state == NULL || !state->linked) {
        return;
    }
    for (link = &hoplite_cosocket_directional_writes;
         *link != NULL;
         link = &(*link)->next)
    {
        if (*link == state) {
            *link = state->next;
            state->next = NULL;
            state->linked = 0;
            return;
        }
    }
    state->linked = 0;
}

static hoplite_cosocket_directional_write_t *
hoplite_cosocket_directional_find(hoplite_cosocket_t *socket)
{
    hoplite_cosocket_directional_write_t *state;

    for (state = hoplite_cosocket_directional_writes;
         state != NULL;
         state = state->next)
    {
        if (!state->released && state->socket == socket) {
            return state;
        }
    }
    return NULL;
}

static hoplite_cosocket_directional_write_t *
hoplite_cosocket_directional_find_connection(ngx_connection_t *connection)
{
    hoplite_cosocket_directional_write_t *state;

    for (state = hoplite_cosocket_directional_writes;
         state != NULL;
         state = state->next)
    {
        if (!state->released && state->active
            && state->connection == connection)
        {
            return state;
        }
    }
    return NULL;
}

static void
hoplite_cosocket_directional_clear(
    hoplite_cosocket_directional_write_t *state,
    ngx_flag_t clear_timer)
{
    ngx_event_t *event;

    if (state == NULL) {
        return;
    }
    if (clear_timer && state->connection != NULL
        && state->connection->write != NULL)
    {
        event = state->connection->write;
        if (event->timer_set) {
            ngx_del_timer(event);
        }
        event->timedout = 0;
    }
    if (state->data != NULL) {
        ngx_free(state->data);
        state->data = NULL;
    }
    state->connection = NULL;
    state->call = 0;
    state->len = 0;
    state->offset = 0;
    state->active = 0;
    ngx_memzero(&state->completer, sizeof(state->completer));
}

static void
hoplite_cosocket_directional_cleanup(void *data)
{
    hoplite_cosocket_directional_write_t *state = data;

    if (state == NULL || state->released) {
        return;
    }
    /*
     * Request cleanup also closes the owning socket. Avoid dereferencing an
     * event that may already have been retired; ngx_close_connection removes
     * its timers. Explicit close/cancel paths clear the live timer first.
     */
    hoplite_cosocket_directional_clear(state, 0);
    hoplite_cosocket_directional_unlink(state);
    state->released = 1;
    state->socket = NULL;
    ngx_free(state);
}

static hoplite_cosocket_directional_write_t *
hoplite_cosocket_directional_get(const hoplite_host_call_v1_t *call,
                                 hoplite_cosocket_t *socket)
{
    hoplite_cosocket_directional_write_t *state;

    state = hoplite_cosocket_directional_find(socket);
    if (state != NULL) {
        return state;
    }
    state = ngx_alloc(sizeof(*state), socket->log);
    if (state == NULL) {
        return NULL;
    }
    ngx_memzero(state, sizeof(*state));
    state->socket = socket;
    state->owner = call->request_context;
    state->work = call->work;
    if (hoplite_host_request_cleanup_add_v1(
            call->request_context,
            call->work,
            state,
            hoplite_cosocket_directional_cleanup) != HOPLITE_HOST_RESOURCE_OK)
    {
        ngx_free(state);
        return NULL;
    }
    state->next = hoplite_cosocket_directional_writes;
    state->linked = 1;
    hoplite_cosocket_directional_writes = state;
    return state;
}

static void
hoplite_cosocket_directional_install(hoplite_cosocket_t *socket)
{
    ngx_connection_t *connection;

    if (socket == NULL || socket->connection == NULL) {
        return;
    }
    connection = socket->connection;
    connection->data = socket;
    connection->read->handler = hoplite_cosocket_directional_read_handler;
    connection->write->handler = hoplite_cosocket_directional_write_handler;
    connection->read->log = socket->log;
    connection->write->log = socket->log;
}

static ngx_int_t
hoplite_cosocket_directional_result(
    ngx_log_t *log,
    ngx_flag_t success,
    int64_t value,
    const char *error,
    hoplite_cosocket_writer_t *writer)
{
    size_t error_len = error == NULL ? 0 : ngx_strlen(error);

    if (hoplite_cosocket_result_writer(writer, error_len, log) != NGX_OK
        || hoplite_cosocket_write_u32(writer, 2) != NGX_OK
        || (success
            ? hoplite_cosocket_write_number(writer, value)
            : hoplite_cosocket_write_nil(writer)) != NGX_OK
        || (success
            ? hoplite_cosocket_write_nil(writer)
            : hoplite_cosocket_write_text(
                writer,
                HOPLITE_HTA_STRING,
                (const u_char *) error,
                error_len)) != NGX_OK)
    {
        if (writer->data != NULL) {
            ngx_free(writer->data);
            writer->data = NULL;
        }
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_directional_prepare(
    hoplite_cosocket_directional_write_t *state,
    ngx_flag_t success,
    int64_t value,
    const char *error,
    ngx_flag_t clear_timer,
    hoplite_cosocket_writer_t *writer,
    hoplite_host_completer_v1_t *completer,
    ngx_log_t **log)
{
    ngx_int_t rc;

    if (state == NULL || !state->active || writer == NULL
        || completer == NULL || log == NULL)
    {
        return NGX_DECLINED;
    }
    *log = state->socket == NULL
        ? hoplite_cosocket_log : state->socket->log;
    rc = hoplite_cosocket_directional_result(
        *log, success, value, error, writer);
    *completer = state->completer;
    hoplite_cosocket_directional_clear(state, clear_timer);
    return rc;
}

static ngx_int_t
hoplite_cosocket_directional_complete(
    hoplite_cosocket_directional_write_t *state,
    ngx_flag_t success,
    int64_t value,
    const char *error,
    ngx_flag_t clear_timer)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};
    hoplite_host_completer_v1_t completer;
    ngx_log_t *log = NULL;

    if (hoplite_cosocket_directional_prepare(
            state,
            success,
            value,
            error,
            clear_timer,
            &writer,
            &completer,
            &log) != NGX_OK)
    {
        return NGX_ERROR;
    }
    return hoplite_cosocket_deliver(&completer, log, 0, &writer);
}

static ngx_int_t
hoplite_cosocket_directional_receive_busy(
    const hoplite_host_call_v1_t *call)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};
    static const u_char empty[] = "";
    const char *error = "socket busy reading";
    size_t error_len = sizeof("socket busy reading") - 1;

    if (hoplite_cosocket_result_writer(
            &writer, error_len, hoplite_cosocket_log) != NGX_OK
        || hoplite_cosocket_write_u32(&writer, 3) != NGX_OK
        || hoplite_cosocket_write_nil(&writer) != NGX_OK
        || hoplite_cosocket_write_text(
            &writer,
            HOPLITE_HTA_STRING,
            (const u_char *) error,
            error_len) != NGX_OK
        || hoplite_cosocket_write_text(
            &writer,
            HOPLITE_HTA_BYTES,
            empty,
            0) != NGX_OK)
    {
        if (writer.data != NULL) {
            ngx_free(writer.data);
        }
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    return hoplite_cosocket_deliver(
        &call->completer, hoplite_cosocket_log, 0, &writer);
}

static ngx_int_t
hoplite_cosocket_directional_prepare_read_failure(
    hoplite_cosocket_t *socket,
    const char *error,
    hoplite_cosocket_writer_t *writer,
    hoplite_host_completer_v1_t *completer,
    ngx_log_t **log)
{
    size_t error_len;
    size_t data_len;
    u_char *data;
    ngx_int_t rc = NGX_OK;

    if (socket == NULL
        || socket->pending != HOPLITE_COSOCKET_PENDING_RECEIVE
        || writer == NULL || completer == NULL || log == NULL)
    {
        return NGX_DECLINED;
    }
    error_len = ngx_strlen(error);
    data_len = socket->receive_len;
    data = socket->receive_data;
    *log = socket->log;
    if (data_len > (size_t) -1 - error_len
        || hoplite_cosocket_result_writer(
            writer, data_len + error_len, *log) != NGX_OK
        || hoplite_cosocket_write_u32(writer, 3) != NGX_OK
        || hoplite_cosocket_write_nil(writer) != NGX_OK
        || hoplite_cosocket_write_text(
            writer,
            HOPLITE_HTA_STRING,
            (const u_char *) error,
            error_len) != NGX_OK
        || hoplite_cosocket_write_text(
            writer,
            HOPLITE_HTA_BYTES,
            data,
            data_len) != NGX_OK)
    {
        if (writer->data != NULL) {
            ngx_free(writer->data);
            writer->data = NULL;
        }
        rc = NGX_ERROR;
    }

    *completer = socket->completer;
    hoplite_cosocket_clear_timer(socket);
    hoplite_cosocket_clear_io(socket);
    ngx_memzero(&socket->completer, sizeof(socket->completer));
    socket->pending = HOPLITE_COSOCKET_PENDING_NONE;
    socket->pending_call = 0;
    return rc;
}

static ngx_int_t
hoplite_cosocket_directional_fail_connection(
    hoplite_cosocket_directional_write_t *state,
    const char *error)
{
    hoplite_cosocket_t *socket;
    hoplite_cosocket_writer_t write_writer = {NULL, 0, 0};
    hoplite_cosocket_writer_t read_writer = {NULL, 0, 0};
    hoplite_host_completer_v1_t write_completer;
    hoplite_host_completer_v1_t read_completer;
    ngx_log_t *write_log = NULL;
    ngx_log_t *read_log = NULL;
    ngx_int_t write_rc;
    ngx_int_t read_rc = NGX_DECLINED;
    ngx_int_t delivery_rc = NGX_OK;

    if (state == NULL || !state->active || state->socket == NULL) {
        return NGX_DECLINED;
    }
    socket = state->socket;
    write_rc = hoplite_cosocket_directional_prepare(
        state,
        0,
        0,
        error,
        1,
        &write_writer,
        &write_completer,
        &write_log);
    if (socket->pending == HOPLITE_COSOCKET_PENDING_RECEIVE) {
        read_rc = hoplite_cosocket_directional_prepare_read_failure(
            socket,
            error,
            &read_writer,
            &read_completer,
            &read_log);
    }
    hoplite_cosocket_reset_connection(socket);

    if (read_rc == NGX_OK
        && hoplite_cosocket_deliver(
            &read_completer, read_log, 0, &read_writer) != NGX_OK)
    {
        delivery_rc = NGX_ERROR;
    }
    if (write_rc == NGX_OK
        && hoplite_cosocket_deliver(
            &write_completer, write_log, 0, &write_writer) != NGX_OK)
    {
        delivery_rc = NGX_ERROR;
    }
    hoplite_cosocket_pool_wake_waiters();
    return write_rc == NGX_OK && delivery_rc == NGX_OK
        ? NGX_OK : NGX_ERROR;
}

static ngx_int_t
hoplite_cosocket_directional_drive(
    hoplite_cosocket_directional_write_t *state)
{
    hoplite_cosocket_t *socket;
    ssize_t sent;

    if (state == NULL || !state->active || state->socket == NULL
        || state->connection == NULL)
    {
        return NGX_ERROR;
    }
    socket = state->socket;
    while (state->offset < state->len) {
        sent = state->connection->send(
            state->connection,
            state->data + state->offset,
            state->len - state->offset);
        if (sent > 0) {
            state->offset += (size_t) sent;
            continue;
        }
        if (sent == NGX_AGAIN) {
            if (socket->send_timeout != 0
                && !state->connection->write->timer_set)
            {
                ngx_add_timer(
                    state->connection->write, socket->send_timeout);
            }
            if (ngx_handle_write_event(
                    state->connection->write, 0) != NGX_OK)
            {
                return hoplite_cosocket_directional_fail_connection(
                    state, "send failed");
            }
            return NGX_AGAIN;
        }
        return hoplite_cosocket_directional_fail_connection(
            state,
            hoplite_cosocket_error_from_errno(
                ngx_socket_errno, HOPLITE_COSOCKET_ERROR_SEND_FAILED));
    }
    return hoplite_cosocket_directional_complete(
        state, 1, (int64_t) state->len, NULL, 1);
}

static ngx_int_t
hoplite_cosocket_directional_send(
    const hoplite_host_call_v1_t *call,
    const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_directional_write_t *state;
    hoplite_cosocket_t *socket;
    ngx_str_t value;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/send expects [socket string-or-bytes]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    state = hoplite_cosocket_directional_find(socket);
    if (state != NULL && state->active) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "socket busy writing");
    }
    if (socket->closed || !socket->connected || socket->connection == NULL
        || hoplite_cosocket_send_is_shutdown(socket))
    {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (hoplite_hta_text(arguments->as.vector.items[1], &value) != NGX_OK
        || value.len > HOPLITE_COSOCKET_MAX_IO)
    {
        return hoplite_cosocket_reject(
            call, "cosocket send value must be at most 1048576 bytes");
    }
    state = hoplite_cosocket_directional_get(call, socket);
    if (state == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (value.len != 0) {
        state->data = ngx_alloc(value.len, socket->log);
        if (state->data == NULL) {
            return HOPLITE_HOST_PROVIDER_ERROR;
        }
        ngx_memcpy(state->data, value.data, value.len);
    }
    state->connection = socket->connection;
    state->call = call->call;
    state->completer = call->completer;
    state->len = value.len;
    state->offset = 0;
    state->active = 1;
    hoplite_cosocket_directional_install(socket);

    rc = hoplite_cosocket_directional_drive(state);
    if (rc == NGX_AGAIN) {
        return HOPLITE_HOST_PROVIDER_PENDING;
    }
    return rc == NGX_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static ngx_flag_t
hoplite_cosocket_directional_read_operation(
    const hoplite_host_call_v1_t *call)
{
    return hoplite_cosocket_operation(call, "receive")
        || hoplite_cosocket_operation(call, "receiveany")
        || hoplite_cosocket_operation(call, "receiveuntil-read");
}

static ngx_int_t
hoplite_cosocket_directional_socket_argument(
    const hoplite_host_call_v1_t *call,
    const hoplite_hta_value_t *arguments,
    hoplite_cosocket_t **socket)
{
    size_t count;

    if (hoplite_cosocket_operation(call, "connect")) {
        count = arguments->as.vector.count;
        if (count != 2 && count != 4) {
            return NGX_ERROR;
        }
    } else if (hoplite_cosocket_operation(call, "receiveuntil-read")) {
        count = 3;
    } else if (hoplite_cosocket_operation(call, "setkeepalive")) {
        count = arguments->as.vector.count;
        if (count < 1 || count > 3) {
            return NGX_ERROR;
        }
    } else if (hoplite_cosocket_operation(call, "setoption")) {
        count = 3;
    } else if (hoplite_cosocket_operation(call, "shutdown")
               || hoplite_cosocket_operation(call, "send")
               || hoplite_cosocket_operation(call, "receive")
               || hoplite_cosocket_operation(call, "receiveany"))
    {
        count = 2;
    } else {
        count = 1;
    }
    return hoplite_cosocket_argument_handle(call, arguments, count, socket);
}

static void
hoplite_cosocket_directional_fail_write_after_read(
    ngx_connection_t *connection)
{
    hoplite_cosocket_directional_write_t *state;
    hoplite_cosocket_t *socket;

    state = hoplite_cosocket_directional_find_connection(connection);
    if (state == NULL || state->socket == NULL) {
        return;
    }
    socket = state->socket;
    if (socket->connection == connection && socket->connected
        && !socket->closed)
    {
        return;
    }
    (void) hoplite_cosocket_directional_complete(
        state, 0, 0, "closed", 0);
}

static void
hoplite_cosocket_directional_write_handler(ngx_event_t *event)
{
    ngx_connection_t *connection =
        event == NULL ? NULL : event->data;
    hoplite_cosocket_directional_write_t *state =
        hoplite_cosocket_directional_find_connection(connection);

    if (state == NULL) {
        hoplite_cosocket_pool_write_handler(event);
        return;
    }
    if (event->timedout) {
        event->timedout = 0;
        (void) hoplite_cosocket_directional_fail_connection(
            state, "timeout");
        return;
    }
    (void) hoplite_cosocket_directional_drive(state);
}

static void
hoplite_cosocket_directional_read_handler(ngx_event_t *event)
{
    ngx_connection_t *connection =
        event == NULL ? NULL : event->data;

    hoplite_cosocket_pool_read_handler(event);
    if (connection != NULL) {
        hoplite_cosocket_directional_fail_write_after_read(connection);
    }
}

static ngx_int_t
hoplite_cosocket_directional_close(
    const hoplite_host_call_v1_t *call,
    const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_directional_write_t *state;
    hoplite_cosocket_t *socket;
    hoplite_cosocket_writer_t write_writer = {NULL, 0, 0};
    hoplite_cosocket_writer_t read_writer = {NULL, 0, 0};
    hoplite_host_completer_v1_t write_completer;
    hoplite_host_completer_v1_t read_completer;
    ngx_log_t *write_log = NULL;
    ngx_log_t *read_log = NULL;
    ngx_int_t write_rc = NGX_DECLINED;
    ngx_int_t read_rc = NGX_DECLINED;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 1, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/close expects [socket]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }

    socket->closed = 1;
    state = hoplite_cosocket_directional_find(socket);
    if (state != NULL && state->active) {
        write_rc = hoplite_cosocket_directional_prepare(
            state,
            0,
            0,
            "closed",
            1,
            &write_writer,
            &write_completer,
            &write_log);
    }
    if (socket->pending == HOPLITE_COSOCKET_PENDING_RECEIVE) {
        read_rc = hoplite_cosocket_directional_prepare_read_failure(
            socket,
            "closed",
            &read_writer,
            &read_completer,
            &read_log);
    }
    hoplite_cosocket_close_state(socket);
    hoplite_cosocket_pool_wake_waiters();

    if (read_rc == NGX_OK) {
        (void) hoplite_cosocket_deliver(
            &read_completer, read_log, 0, &read_writer);
    }
    if (write_rc == NGX_OK) {
        (void) hoplite_cosocket_deliver(
            &write_completer, write_log, 0, &write_writer);
    }
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static int32_t
hoplite_cosocket_directional_invoke(const hoplite_host_call_v1_t *call)
{
    hoplite_cosocket_directional_write_t *state;
    hoplite_hta_value_t *arguments = NULL;
    hoplite_cosocket_t *socket = NULL;
    ngx_pool_t *pool;
    ngx_int_t argument_rc, rc;

    if (call == NULL
        || call->abi_version != HOPLITE_HOST_PROVIDER_ABI_VERSION
        || call->request_context == NULL
        || call->work == 0 || call->call == 0
        || call->operation.data == NULL || call->operation.len == 0
        || call->completer.succeed == NULL || call->completer.fail == NULL)
    {
        return HOPLITE_HOST_PROVIDER_ERROR;
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

    if (hoplite_cosocket_operation(call, "send")) {
        rc = hoplite_cosocket_directional_send(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "close")) {
        rc = hoplite_cosocket_directional_close(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }

    if (hoplite_cosocket_directional_read_operation(call)
        || hoplite_cosocket_operation(call, "connect")
        || hoplite_cosocket_operation(call, "shutdown")
        || hoplite_cosocket_operation(call, "setkeepalive"))
    {
        argument_rc = hoplite_cosocket_directional_socket_argument(
            call, arguments, &socket);
        if (argument_rc == NGX_OK) {
            state = hoplite_cosocket_directional_find(socket);
            if (hoplite_cosocket_directional_read_operation(call)
                && socket->pending == HOPLITE_COSOCKET_PENDING_RECEIVE)
            {
                ngx_destroy_pool(pool);
                return (int32_t)
                    hoplite_cosocket_directional_receive_busy(call);
            }
            if ((hoplite_cosocket_operation(call, "shutdown")
                 || hoplite_cosocket_operation(call, "setkeepalive"))
                && state != NULL && state->active)
            {
                rc = hoplite_cosocket_complete_ordinary_call(
                    call,
                    0,
                    0,
                    hoplite_cosocket_operation(call, "shutdown")
                        ? "socket busy writing"
                        : "connection in dubious state");
                ngx_destroy_pool(pool);
                return (int32_t) rc;
            }
            if (socket->connection != NULL) {
                hoplite_cosocket_directional_install(socket);
            }
        }
    }

    rc = hoplite_cosocket_invoke(call);

    socket = NULL;
    if (hoplite_cosocket_directional_read_operation(call)
        || hoplite_cosocket_operation(call, "connect")
        || hoplite_cosocket_operation(call, "shutdown")
        || hoplite_cosocket_operation(call, "setkeepalive"))
    {
        argument_rc = hoplite_cosocket_directional_socket_argument(
            call, arguments, &socket);
        if (argument_rc != NGX_OK) {
            socket = NULL;
        }
    }
    if (socket != NULL) {
        state = hoplite_cosocket_directional_find(socket);
        if (socket->connection != NULL) {
            hoplite_cosocket_directional_install(socket);
        } else if (state != NULL && state->active) {
            (void) hoplite_cosocket_directional_complete(
                state, 0, 0, "closed", 0);
        }
    }
    ngx_destroy_pool(pool);
    return (int32_t) rc;
}

static void
hoplite_cosocket_directional_cancel(void *request_context)
{
    hoplite_cosocket_directional_write_t *state;

    for (state = hoplite_cosocket_directional_writes;
         state != NULL;
         state = state->next)
    {
        if (!state->released && state->owner == request_context
            && state->active)
        {
            hoplite_cosocket_directional_clear(state, 1);
        }
    }
    hoplite_cosocket_cancel(request_context);
}

ngx_int_t
hoplite_cosocket_directional_register(ngx_cycle_t *cycle)
{
    hoplite_cosocket_directional_writes = NULL;
    return hoplite_cosocket_register(cycle);
}

void
hoplite_cosocket_directional_worker_exit(void)
{
    hoplite_cosocket_directional_write_t *state;

    while (hoplite_cosocket_directional_writes != NULL) {
        state = hoplite_cosocket_directional_writes;
        hoplite_cosocket_directional_writes = state->next;
        state->next = NULL;
        state->linked = 0;
        if (!state->released) {
            hoplite_cosocket_directional_clear(state, 1);
            state->released = 1;
            state->socket = NULL;
            ngx_free(state);
        }
    }
    hoplite_cosocket_worker_exit();
}
