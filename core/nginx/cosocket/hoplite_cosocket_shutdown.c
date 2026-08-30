#include "hoplite_cosocket.h"

#include <limits.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <sys/un.h>

#include <ngx_http.h>

/*
 * Keep the established TCP implementation in one translation unit while these
 * compatibility slices extend provider dispatch with send-direction shutdown,
 * the bounded OpenResty/LuaSocket setoption surface, Unix-domain stream
 * connections, Nginx-resolver-backed hostnames, worker-local keepalive pools,
 * and bounded FIFO connect backlog. The included core keeps worker lifecycle
 * and request-owned state; only its provider entry points are renamed so this
 * wrapper can add operations without duplicating the event-loop implementation.
 */
#define hoplite_cosocket_provider hoplite_cosocket_core_provider
#define hoplite_cosocket_invoke hoplite_cosocket_core_invoke
#define hoplite_cosocket_cancel hoplite_cosocket_core_cancel
#define hoplite_cosocket_register hoplite_cosocket_core_register
#define hoplite_cosocket_worker_exit hoplite_cosocket_core_worker_exit
#include "hoplite_cosocket.c"
#undef hoplite_cosocket_worker_exit
#undef hoplite_cosocket_register
#undef hoplite_cosocket_cancel
#undef hoplite_cosocket_invoke
#undef hoplite_cosocket_provider

typedef struct hoplite_cosocket_resolution_s
    hoplite_cosocket_resolution_t;

struct hoplite_cosocket_resolution_s {
    hoplite_cosocket_t *socket;
    ngx_resolver_ctx_t *ctx;
    ngx_str_t host;
    in_port_t port;
    ngx_msec_t started;
    ngx_flag_t linked;
    ngx_flag_t done;
    hoplite_cosocket_resolution_t *next;
};

static ngx_resolver_t *hoplite_cosocket_resolver;
static ngx_msec_t hoplite_cosocket_resolver_timeout;
static hoplite_cosocket_resolution_t *hoplite_cosocket_resolutions;

/* Implemented by hoplite_cosocket_pool.inc below. */
static void hoplite_cosocket_pool_wake_waiters(void);
static void hoplite_cosocket_pool_read_handler(ngx_event_t *event);
static void hoplite_cosocket_pool_write_handler(ngx_event_t *event);

static void
hoplite_cosocket_resolution_unlink(hoplite_cosocket_resolution_t *state)
{
    hoplite_cosocket_resolution_t **link;

    if (state == NULL || !state->linked) {
        return;
    }
    for (link = &hoplite_cosocket_resolutions;
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

static void
hoplite_cosocket_resolution_finish(hoplite_cosocket_resolution_t *state)
{
    ngx_resolver_ctx_t *ctx;

    if (state == NULL || state->done) {
        return;
    }
    ctx = state->ctx;
    state->ctx = NULL;
    state->done = 1;
    hoplite_cosocket_resolution_unlink(state);
    if (ctx != NULL) {
        ngx_resolve_name_done(ctx);
    }
}

static void
hoplite_cosocket_resolution_cleanup(void *data)
{
    hoplite_cosocket_resolution_t *state = data;

    if (state == NULL) {
        return;
    }
    hoplite_cosocket_resolution_finish(state);
    state->socket = NULL;
    if (state->host.data != NULL) {
        ngx_free(state->host.data);
        state->host.data = NULL;
        state->host.len = 0;
    }
    ngx_free(state);
}

static void
hoplite_cosocket_resolver_configure(ngx_cycle_t *cycle)
{
    ngx_http_conf_ctx_t *http;
    ngx_http_core_loc_conf_t *core;

    hoplite_cosocket_resolver = NULL;
    hoplite_cosocket_resolver_timeout = 0;
    if (cycle == NULL || cycle->conf_ctx == NULL
        || ngx_http_module.index == NGX_MODULE_UNSET_INDEX)
    {
        return;
    }
    http = (ngx_http_conf_ctx_t *)
        cycle->conf_ctx[ngx_http_module.index];
    if (http == NULL || http->loc_conf == NULL) {
        return;
    }
    core = http->loc_conf[ngx_http_core_module.ctx_index];
    if (core == NULL) {
        return;
    }
    hoplite_cosocket_resolver = core->resolver;
    hoplite_cosocket_resolver_timeout = core->resolver_timeout;
}

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
        message = hoplite_cosocket_error_from_errno(
            error, HOPLITE_COSOCKET_ERROR_SHUTDOWN_FAILED);
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
        message = hoplite_cosocket_error_from_errno(
            error, HOPLITE_COSOCKET_ERROR_SOCKET_OPTION_FAILED);
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, message);
    }
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_connect_peer(hoplite_cosocket_t *socket,
                              struct sockaddr *sockaddr,
                              socklen_t socklen,
                              ngx_str_t *name,
                              ngx_msec_t timeout)
{
    ngx_str_t *retained_name;
    ngx_int_t rc;

    if (name == NULL || name->data == NULL || name->len == 0) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    retained_name = ngx_palloc(socket->pool, sizeof(*retained_name));
    if (retained_name == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    retained_name->data = ngx_pnalloc(socket->pool, name->len);
    if (retained_name->data == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ngx_memcpy(retained_name->data, name->data, name->len);
    retained_name->len = name->len;

    ngx_memzero(&socket->peer, sizeof(socket->peer));
    socket->peer.sockaddr = sockaddr;
    socket->peer.socklen = socklen;
    socket->peer.name = retained_name;
    socket->peer.get = ngx_event_get_peer;
    socket->peer.data = socket;
    socket->peer.log = socket->log;
    socket->peer.log_error = NGX_ERROR_ERR;
    socket->peer.type = SOCK_STREAM;
    socket->peer.tries = 1;

    rc = ngx_event_connect_peer(&socket->peer);
    if (rc == NGX_ERROR || rc == NGX_DECLINED || rc == NGX_BUSY
        || socket->peer.connection == NULL)
    {
        socket->connection = socket->peer.connection;
        hoplite_cosocket_reset_connection(socket);
        rc = hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "connect failed");
        hoplite_cosocket_pool_wake_waiters();
        return rc;
    }
    socket->connection = socket->peer.connection;
    socket->connection->data = socket;
    socket->connection->read->handler = hoplite_cosocket_pool_read_handler;
    socket->connection->write->handler = hoplite_cosocket_pool_write_handler;
    socket->connection->read->log = socket->log;
    socket->connection->write->log = socket->log;
    if (socket->connection->pool == NULL) {
        socket->connection->pool = socket->pool;
    }

    if (rc == NGX_AGAIN) {
        if (timeout != 0) {
            ngx_add_timer(socket->connection->write, timeout);
        }
        return HOPLITE_HOST_PROVIDER_PENDING;
    }
    socket->connected = 1;
    return hoplite_cosocket_complete_ordinary(socket, 1, 1, NULL)
        == NGX_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
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

    socket->pending = HOPLITE_COSOCKET_PENDING_CONNECT;
    socket->pending_call = call->call;
    socket->completer = call->completer;
    return hoplite_cosocket_connect_peer(socket,
                                         url.addrs[0].sockaddr,
                                         url.addrs[0].socklen,
                                         &url.addrs[0].name,
                                         socket->connect_timeout);
}

static ngx_int_t
hoplite_cosocket_resolved_compare(const ngx_resolver_addr_t *left,
                                  const ngx_resolver_addr_t *right)
{
    sa_family_t left_family, right_family;
    int comparison;

    left_family = left->sockaddr->sa_family;
    right_family = right->sockaddr->sa_family;
    if (left_family != right_family) {
        return left_family < right_family ? -1 : 1;
    }
    if (left_family == AF_INET) {
        const struct sockaddr_in *left_in;
        const struct sockaddr_in *right_in;
        left_in = (const struct sockaddr_in *) left->sockaddr;
        right_in = (const struct sockaddr_in *) right->sockaddr;
        if (left_in->sin_addr.s_addr == right_in->sin_addr.s_addr) {
            return 0;
        }
        return ntohl(left_in->sin_addr.s_addr)
            < ntohl(right_in->sin_addr.s_addr) ? -1 : 1;
    }
#if (NGX_HAVE_INET6)
    if (left_family == AF_INET6) {
        const struct sockaddr_in6 *left_in6;
        const struct sockaddr_in6 *right_in6;
        left_in6 = (const struct sockaddr_in6 *) left->sockaddr;
        right_in6 = (const struct sockaddr_in6 *) right->sockaddr;
        comparison = ngx_memcmp(&left_in6->sin6_addr,
                                &right_in6->sin6_addr,
                                sizeof(struct in6_addr));
        return comparison < 0 ? -1 : comparison > 0 ? 1 : 0;
    }
#endif
    if (left->socklen != right->socklen) {
        return left->socklen < right->socklen ? -1 : 1;
    }
    comparison = ngx_memcmp(left->sockaddr,
                            right->sockaddr,
                            left->socklen);
    return comparison < 0 ? -1 : comparison > 0 ? 1 : 0;
}

static const char *
hoplite_cosocket_resolver_error(ngx_int_t state)
{
    switch (state) {
    case NGX_RESOLVE_NXDOMAIN:
        return "host not found";
    case NGX_RESOLVE_TIMEDOUT:
        return "timeout";
    case NGX_RESOLVE_REFUSED:
        return "resolver refused";
    case NGX_RESOLVE_SERVFAIL:
        return "resolver failure";
    default:
        return "name resolution failed";
    }
}

static void
hoplite_cosocket_resolve_handler(ngx_resolver_ctx_t *ctx)
{
    hoplite_cosocket_resolution_t *state;
    hoplite_cosocket_t *socket;
    ngx_resolver_addr_t *selected;
    struct sockaddr *sockaddr;
    socklen_t socklen;
    ngx_msec_t elapsed, remaining;
    ngx_uint_t index;

    state = ctx == NULL ? NULL : ctx->data;
    socket = state == NULL ? NULL : state->socket;
    if (state == NULL || state->done || socket == NULL
        || socket->released || socket->closed
        || socket->pending != HOPLITE_COSOCKET_PENDING_CONNECT)
    {
        if (state != NULL) {
            hoplite_cosocket_resolution_finish(state);
        } else if (ctx != NULL) {
            ngx_resolve_name_done(ctx);
        }
        hoplite_cosocket_pool_wake_waiters();
        return;
    }

    if (ctx->state != NGX_OK || ctx->naddrs == 0 || ctx->addrs == NULL) {
        const char *error = hoplite_cosocket_resolver_error(ctx->state);
        hoplite_cosocket_resolution_finish(state);
        (void) hoplite_cosocket_complete_ordinary(socket, 0, 0, error);
        hoplite_cosocket_pool_wake_waiters();
        return;
    }

    selected = &ctx->addrs[0];
    for (index = 1; index < ctx->naddrs; index++) {
        if (hoplite_cosocket_resolved_compare(&ctx->addrs[index], selected) < 0) {
            selected = &ctx->addrs[index];
        }
    }
    socklen = selected->socklen;
    sockaddr = ngx_pnalloc(socket->pool, socklen);
    if (sockaddr == NULL) {
        hoplite_cosocket_resolution_finish(state);
        (void) hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "could not retain resolved address");
        hoplite_cosocket_pool_wake_waiters();
        return;
    }
    ngx_memcpy(sockaddr, selected->sockaddr, socklen);
    ngx_inet_set_port(sockaddr, state->port);

    remaining = socket->connect_timeout;
    if (remaining != 0) {
        elapsed = ngx_current_msec - state->started;
        if (elapsed >= remaining) {
            hoplite_cosocket_resolution_finish(state);
            (void) hoplite_cosocket_complete_ordinary(
                socket, 0, 0, "timeout");
            hoplite_cosocket_pool_wake_waiters();
            return;
        }
        remaining -= elapsed;
    }

    hoplite_cosocket_resolution_finish(state);
    (void) hoplite_cosocket_connect_peer(socket,
                                         sockaddr,
                                         socklen,
                                         &state->host,
                                         remaining);
}

static ngx_int_t
hoplite_cosocket_connect_hostname(const hoplite_host_call_v1_t *call,
                                  const hoplite_hta_value_t *arguments,
                                  ngx_pool_t *pool)
{
    hoplite_cosocket_resolution_t *state;
    hoplite_cosocket_t *socket;
    hoplite_hta_value_t *options;
    ngx_resolver_ctx_t temp, *ctx;
    ngx_addr_t numeric;
    ngx_str_t host;
    ngx_msec_t timeout;
    int64_t port;
    ngx_int_t rc;
    size_t index;

    rc = hoplite_cosocket_argument_handle(call, arguments, 4, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/connect expects [socket host port options]");
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
    if (hoplite_hta_text(arguments->as.vector.items[1], &host) != NGX_OK
        || host.len == 0 || host.len > 255
        || hoplite_hta_number(arguments->as.vector.items[2], &port) != NGX_OK
        || port < 1 || port > 65535)
    {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/connect requires a host and port 1..65535");
    }
    options = arguments->as.vector.items[3];
    if (options == NULL
        || (options->kind != HOPLITE_HTA_MAP
            && options->kind != HOPLITE_HTA_OBJECT)
        || options->as.map.count != 0)
    {
        return hoplite_cosocket_reject(
            call, "cosocket connect options are reserved for pooling");
    }
    if (ngx_parse_addr(pool, &numeric, host.data, host.len) == NGX_OK) {
        return NGX_DECLINED;
    }
    for (index = 0; index < host.len; index++) {
        if (host.data[index] == '\0'
            || host.data[index] == '/'
            || host.data[index] == ':'
            || host.data[index] == ' '
            || host.data[index] == '\t'
            || host.data[index] == '\r'
            || host.data[index] == '\n')
        {
            return hoplite_cosocket_reject(
                call, "hostname contains an unsupported character");
        }
    }

    state = ngx_alloc(sizeof(*state), socket->log);
    if (state == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ngx_memzero(state, sizeof(*state));
    state->host.data = ngx_alloc(host.len, socket->log);
    if (state->host.data == NULL) {
        ngx_free(state);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ngx_memcpy(state->host.data, host.data, host.len);
    state->host.len = host.len;
    state->socket = socket;
    state->port = (in_port_t) port;
    state->started = ngx_current_msec;
    if (hoplite_host_request_cleanup_add_v1(
            call->request_context,
            call->work,
            state,
            hoplite_cosocket_resolution_cleanup) != HOPLITE_HOST_RESOURCE_OK)
    {
        ngx_free(state->host.data);
        ngx_free(state);
        return hoplite_cosocket_reject(
            call, "could not register resolver cleanup");
    }
    state->next = hoplite_cosocket_resolutions;
    state->linked = 1;
    hoplite_cosocket_resolutions = state;

    socket->pending = HOPLITE_COSOCKET_PENDING_CONNECT;
    socket->pending_call = call->call;
    socket->completer = call->completer;

    if (hoplite_cosocket_resolver == NULL) {
        hoplite_cosocket_resolution_finish(state);
        return hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "resolver not configured");
    }
    ngx_memzero(&temp, sizeof(temp));
    temp.name = state->host;
    ctx = ngx_resolve_start(hoplite_cosocket_resolver, &temp);
    if (ctx == NGX_NO_RESOLVER) {
        hoplite_cosocket_resolution_finish(state);
        return hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "resolver not configured");
    }
    if (ctx == NULL) {
        hoplite_cosocket_resolution_finish(state);
        return hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "resolver unavailable");
    }
    state->ctx = ctx;
    ctx->name = state->host;
    ctx->handler = hoplite_cosocket_resolve_handler;
    ctx->data = state;
    timeout = hoplite_cosocket_resolver_timeout;
    if (socket->connect_timeout != 0
        && (timeout == 0 || socket->connect_timeout < timeout))
    {
        timeout = socket->connect_timeout;
    }
    if (timeout == 0) {
        timeout = HOPLITE_COSOCKET_DEFAULT_TIMEOUT;
    }
    ctx->timeout = timeout;
    if (ngx_resolve_name(ctx) != NGX_OK) {
        state->ctx = NULL;
        state->done = 1;
        hoplite_cosocket_resolution_unlink(state);
        return hoplite_cosocket_complete_ordinary(
            socket, 0, 0, "name resolution failed");
    }
    return socket->pending == HOPLITE_COSOCKET_PENDING_NONE
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_PENDING;
}

#include "hoplite_cosocket_pool.inc"

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
        && !hoplite_cosocket_operation(call, "receive")
        && !hoplite_cosocket_operation(call, "receiveany")
        && !hoplite_cosocket_operation(call, "receiveuntil-read")
        && !hoplite_cosocket_operation(call, "close")
        && !hoplite_cosocket_operation(call, "setoption")
        && !hoplite_cosocket_operation(call, "connect")
        && !hoplite_cosocket_operation(call, "setkeepalive")
        && !hoplite_cosocket_operation(call, "getreusedtimes"))
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
        rc = hoplite_cosocket_pool_connect(call, arguments, pool);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "setkeepalive")) {
        rc = hoplite_cosocket_pool_setkeepalive(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "getreusedtimes")) {
        rc = hoplite_cosocket_pool_getreusedtimes(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "close")) {
        rc = hoplite_cosocket_close(call, arguments);
        ngx_destroy_pool(pool);
        hoplite_cosocket_pool_wake_waiters();
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "shutdown")) {
        rc = hoplite_cosocket_shutdown(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }
    if (hoplite_cosocket_operation(call, "setoption")) {
        hoplite_cosocket_pool_mark_optioned(call, arguments);
        rc = hoplite_cosocket_setoption(call, arguments);
        ngx_destroy_pool(pool);
        return (int32_t) rc;
    }

    if (hoplite_cosocket_operation(call, "send")) {
        rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
        if (rc == NGX_OK && hoplite_cosocket_send_is_shutdown(socket)) {
            rc = hoplite_cosocket_complete_ordinary_call(
                call, 0, 0, "closed");
            ngx_destroy_pool(pool);
            return (int32_t) rc;
        }
    }

    ngx_destroy_pool(pool);
    rc = hoplite_cosocket_core_invoke(call);
    hoplite_cosocket_pool_wake_waiters();
    return (int32_t) rc;
}

static void
hoplite_cosocket_cancel(void *request_context)
{
    hoplite_cosocket_resolution_t *state, *next;

    hoplite_cosocket_pool_cancel(request_context);
    for (state = hoplite_cosocket_resolutions;
         state != NULL;
         state = next)
    {
        next = state->next;
        if (state->socket != NULL
            && state->socket->owner == request_context)
        {
            hoplite_cosocket_resolution_finish(state);
        }
    }
    hoplite_cosocket_core_cancel(request_context);
    hoplite_cosocket_pool_wake_waiters();
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
    hoplite_cosocket_resolutions = NULL;
    hoplite_cosocket_pool_init();
    hoplite_cosocket_resolver_configure(cycle);
    rc = hoplite_host_provider_register_v1(&hoplite_cosocket_provider);
    return rc == HOPLITE_HOST_PROVIDER_REGISTER_OK ? NGX_OK : NGX_ERROR;
}

void
hoplite_cosocket_worker_exit(void)
{
    hoplite_cosocket_resolution_t *state, *next;

    hoplite_cosocket_pool_worker_exit();
    for (state = hoplite_cosocket_resolutions;
         state != NULL;
         state = next)
    {
        next = state->next;
        hoplite_cosocket_resolution_finish(state);
        state->socket = NULL;
    }
    hoplite_cosocket_resolutions = NULL;
    hoplite_cosocket_resolver = NULL;
    hoplite_cosocket_resolver_timeout = 0;
    hoplite_cosocket_core_worker_exit();
}
