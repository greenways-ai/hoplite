#include "hoplite_cosocket.h"

#include <limits.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <sys/un.h>

/*
 * Keep the established TCP implementation in one translation unit while these
 * compatibility slices extend provider dispatch with send-direction shutdown,
 * the bounded OpenResty/LuaSocket setoption surface, and Unix-domain stream
 * connections. The included core keeps worker lifecycle and request-owned
 * state; only its provider entry points are renamed so this wrapper can add
 * operations without duplicating the event-loop implementation.
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

static ngx_flag_t
hoplite_cosocket_option_is(const ngx_str_t *option, const char *name)
{
    size_t len;

    if (option == NULL || name == NULL) {
        return 0;
    }
    len = ngx_strlen(name);
    return option->len == len
        && ngx_strncmp(option->data, name, len) == 0;
}

static ngx_int_t
hoplite_cosocket_option_boolean(const hoplite_hta_value_t *value, int *output)
{
    int64_t number;

    if (value == NULL || output == NULL) {
        return NGX_ERROR;
    }
    if (value->kind == HOPLITE_HTA_BOOL) {
        *output = value->as.boolean ? 1 : 0;
        return NGX_OK;
    }
    if (hoplite_hta_number(value, &number) == NGX_OK
        && (number == 0 || number == 1))
    {
        *output = (int) number;
        return NGX_OK;
    }
    return NGX_ERROR;
}

static ngx_int_t
hoplite_cosocket_option_buffer(const hoplite_hta_value_t *value, int *output)
{
    int64_t number;

    if (value == NULL || output == NULL
        || hoplite_hta_number(value, &number) != NGX_OK
        || number < 0 || number > INT_MAX)
    {
        return NGX_ERROR;
    }
    *output = (int) number;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_setoption(const hoplite_host_call_v1_t *call,
                           const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    ngx_str_t option;
    int level, name, value;
    ngx_err_t error;
    const char *message;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 3, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/setoption expects [socket option value]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed || !socket->connected
        || socket->connection == NULL)
    {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (hoplite_hta_text(arguments->as.vector.items[1], &option) != NGX_OK) {
        return hoplite_cosocket_reject(
            call, "cosocket setoption name must be text");
    }

    if (hoplite_cosocket_option_is(&option, "keepalive")) {
        level = SOL_SOCKET;
        name = SO_KEEPALIVE;
        if (hoplite_cosocket_option_boolean(
                arguments->as.vector.items[2], &value) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "keepalive must be boolean or 0/1");
        }
    } else if (hoplite_cosocket_option_is(&option, "reuseaddr")) {
        level = SOL_SOCKET;
        name = SO_REUSEADDR;
        if (hoplite_cosocket_option_boolean(
                arguments->as.vector.items[2], &value) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "reuseaddr must be boolean or 0/1");
        }
    } else if (hoplite_cosocket_option_is(&option, "tcp-nodelay")) {
        level = IPPROTO_TCP;
        name = TCP_NODELAY;
        if (hoplite_cosocket_option_boolean(
                arguments->as.vector.items[2], &value) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "tcp-nodelay must be boolean or 0/1");
        }
    } else if (hoplite_cosocket_option_is(&option, "sndbuf")) {
        level = SOL_SOCKET;
        name = SO_SNDBUF;
        if (hoplite_cosocket_option_buffer(
                arguments->as.vector.items[2], &value) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "sndbuf must be an integer from 0 through INT_MAX");
        }
    } else if (hoplite_cosocket_option_is(&option, "rcvbuf")) {
        level = SOL_SOCKET;
        name = SO_RCVBUF;
        if (hoplite_cosocket_option_buffer(
                arguments->as.vector.items[2], &value) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "rcvbuf must be an integer from 0 through INT_MAX");
        }
    } else {
        return hoplite_cosocket_reject(
            call,
            "setoption supports keepalive, reuseaddr, tcp-nodelay, sndbuf, or rcvbuf");
    }

    if (setsockopt(socket->connection->fd,
                   level,
                   name,
                   (const void *) &value,
                   sizeof(value)) == -1)
    {
        error = ngx_socket_errno;
        message = hoplite_cosocket_error_text(error, "setsockopt failed");
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, message);
    }
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_connect_unix(const hoplite_host_call_v1_t *call,
                              const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    ngx_url_t url;
    ngx_str_t target;
    ngx_int_t rc;
    size_t index, path_len;

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/connect expects [socket unix-target]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (socket->connected || socket->connection != NULL) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "already connected");
    }
    if (hoplite_hta_text(arguments->as.vector.items[1], &target) != NGX_OK
        || target.len < sizeof("unix:/") - 1
        || ngx_strncmp(target.data,
                       "unix:/",
                       sizeof("unix:/") - 1) != 0)
    {
        return hoplite_cosocket_reject(
            call, "Unix-domain target must be unix:/absolute/path");
    }

    path_len = target.len - (sizeof("unix:") - 1);
    if (path_len >= sizeof(((struct sockaddr_un *) 0)->sun_path)) {
        return hoplite_cosocket_reject(
            call, "Unix-domain socket path is too long");
    }
    for (index = 0; index < target.len; index++) {
        if (target.data[index] == '\0') {
            return hoplite_cosocket_reject(
                call, "Unix-domain target cannot contain a NUL byte");
        }
    }

    ngx_memzero(&url, sizeof(url));
    url.url.data = ngx_pnalloc(socket->pool, target.len + 1);
    if (url.url.data == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ngx_memcpy(url.url.data, target.data, target.len);
    url.url.data[target.len] = '\0';
    url.url.len = target.len;
    url.no_resolve = 1;
    if (ngx_parse_url(socket->pool, &url) != NGX_OK
        || url.naddrs != 1
        || url.addrs == NULL
        || url.addrs[0].sockaddr == NULL
        || url.addrs[0].sockaddr->sa_family != AF_UNIX)
    {
        return hoplite_cosocket_reject(
            call, "invalid Unix-domain socket target");
    }

    ngx_memzero(&socket->peer, sizeof(socket->peer));
    socket->peer.sockaddr = url.addrs[0].sockaddr;
    socket->peer.socklen = url.addrs[0].socklen;
    socket->peer.name = &url.addrs[0].name;
    socket->peer.get = ngx_event_get_peer;
    socket->peer.data = socket;
    socket->peer.log = socket->log;
    socket->peer.log_error = NGX_ERROR_ERR;
    socket->peer.type = SOCK_STREAM;
    socket->peer.tries = 1;
    socket->pending = HOPLITE_COSOCKET_PENDING_CONNECT;
    socket->pending_call = call->call;
    socket->completer = call->completer;

    rc = ngx_event_connect_peer(&socket->peer);
    if (rc == NGX_ERROR || rc == NGX_DECLINED || rc == NGX_BUSY
        || socket->peer.connection == NULL)
    {
        socket->connection = socket->peer.connection;
        hoplite_cosocket_reset_connection(socket);
        return hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "connect failed");
    }
    socket->connection = socket->peer.connection;
    socket->connection->data = socket;
    socket->connection->read->handler = hoplite_cosocket_read_handler;
    socket->connection->write->handler = hoplite_cosocket_write_handler;
    socket->connection->read->log = socket->log;
    socket->connection->write->log = socket->log;
    if (socket->connection->pool == NULL) {
        socket->connection->pool = socket->pool;
    }

    if (rc == NGX_AGAIN) {
        if (socket->connect_timeout != 0) {
            ngx_add_timer(socket->connection->write,
                          socket->connect_timeout);
        }
        return HOPLITE_HOST_PROVIDER_PENDING;
    }
    socket->connected = 1;
    if (hoplite_cosocket_complete_ordinary(socket, 1, 1, NULL) != NGX_OK) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    return HOPLITE_HOST_PROVIDER_OK;
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
        && !hoplite_cosocket_operation(call, "send")
        && !hoplite_cosocket_operation(call, "setoption")
        && !hoplite_cosocket_operation(call, "connect"))
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

    if (hoplite_cosocket_operation(call, "connect")) {
        if (arguments->as.vector.count == 2) {
            rc = hoplite_cosocket_connect_unix(call, arguments);
            ngx_destroy_pool(pool);
            return (int32_t) rc;
        }
        ngx_destroy_pool(pool);
        return hoplite_cosocket_core_invoke(call);
    }
    if (hoplite_cosocket_operation(call, "shutdown")) {
        rc = hoplite_cosocket_shutdown(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "setoption")) {
        rc = hoplite_cosocket_setoption(call, arguments);
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
