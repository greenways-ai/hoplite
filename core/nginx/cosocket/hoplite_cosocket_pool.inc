#ifndef HOPLITE_COSOCKET_POOL_INC
#define HOPLITE_COSOCKET_POOL_INC

#define HOPLITE_COSOCKET_POOL_DEFAULT_SIZE 30u
#define HOPLITE_COSOCKET_POOL_MAX_SIZE 1024u
#define HOPLITE_COSOCKET_POOL_DEFAULT_TIMEOUT 60000
#define HOPLITE_COSOCKET_POOL_MAX_KEY 1024u

typedef struct hoplite_cosocket_pool_active_s
    hoplite_cosocket_pool_active_t;
typedef struct hoplite_cosocket_pool_idle_s
    hoplite_cosocket_pool_idle_t;

typedef struct {
    ngx_str_t key;
    ngx_uint_t size;
    ngx_flag_t numeric;
} hoplite_cosocket_pool_spec_t;

struct hoplite_cosocket_pool_active_s {
    hoplite_cosocket_t *socket;
    u_char *key;
    size_t key_len;
    ngx_uint_t size;
    uint64_t reused;
    ngx_flag_t poolable;
    ngx_flag_t linked;
    ngx_flag_t released;
    hoplite_cosocket_pool_active_t *next;
};

struct hoplite_cosocket_pool_idle_s {
    ngx_connection_t *connection;
    ngx_pool_t *pool;
    u_char *key;
    size_t key_len;
    ngx_msec_t expires;
    uint64_t reused;
    uint64_t serial;
    ngx_flag_t linked;
    hoplite_cosocket_pool_idle_t *next;
};

static hoplite_cosocket_pool_active_t *hoplite_cosocket_pool_active;
static hoplite_cosocket_pool_idle_t *hoplite_cosocket_pool_idle;
static uint64_t hoplite_cosocket_pool_serial;

static ngx_flag_t
hoplite_cosocket_pool_key_equal(const u_char *left,
                                size_t left_len,
                                const u_char *right,
                                size_t right_len)
{
    return left_len == right_len
        && (left_len == 0 || ngx_memcmp(left, right, left_len) == 0);
}

static void
hoplite_cosocket_pool_active_unlink(hoplite_cosocket_pool_active_t *state)
{
    hoplite_cosocket_pool_active_t **link;

    if (state == NULL || !state->linked) {
        return;
    }
    for (link = &hoplite_cosocket_pool_active;
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
hoplite_cosocket_pool_active_cleanup(void *data)
{
    hoplite_cosocket_pool_active_t *state = data;

    if (state == NULL || state->released) {
        return;
    }
    state->released = 1;
    hoplite_cosocket_pool_active_unlink(state);
    if (state->key != NULL) {
        ngx_free(state->key);
        state->key = NULL;
    }
    state->socket = NULL;
    ngx_free(state);
}

static hoplite_cosocket_pool_active_t *
hoplite_cosocket_pool_active_find(hoplite_cosocket_t *socket)
{
    hoplite_cosocket_pool_active_t *state;

    for (state = hoplite_cosocket_pool_active;
         state != NULL;
         state = state->next)
    {
        if (!state->released && state->socket == socket) {
            return state;
        }
    }
    return NULL;
}

static void
hoplite_cosocket_pool_active_retire(hoplite_cosocket_pool_active_t *state,
                                    ngx_flag_t keep_key)
{
    if (state == NULL) {
        return;
    }
    hoplite_cosocket_pool_active_unlink(state);
    state->socket = NULL;
    if (!keep_key && state->key != NULL) {
        ngx_free(state->key);
        state->key = NULL;
        state->key_len = 0;
    }
}

static ngx_int_t
hoplite_cosocket_pool_active_add(const hoplite_host_call_v1_t *call,
                                 hoplite_cosocket_t *socket,
                                 const u_char *key,
                                 size_t key_len,
                                 ngx_uint_t size,
                                 uint64_t reused)
{
    hoplite_cosocket_pool_active_t *previous, *state;

    if (call == NULL || socket == NULL || key == NULL || key_len == 0) {
        return NGX_ERROR;
    }
    previous = hoplite_cosocket_pool_active_find(socket);
    if (previous != NULL) {
        hoplite_cosocket_pool_active_retire(previous, 0);
    }

    state = ngx_alloc(sizeof(*state), socket->log);
    if (state == NULL) {
        return NGX_ERROR;
    }
    ngx_memzero(state, sizeof(*state));
    state->key = ngx_alloc(key_len, socket->log);
    if (state->key == NULL) {
        ngx_free(state);
        return NGX_ERROR;
    }
    ngx_memcpy(state->key, key, key_len);
    state->key_len = key_len;
    state->socket = socket;
    state->size = size;
    state->reused = reused;
    state->poolable = 1;
    if (hoplite_host_request_cleanup_add_v1(
            call->request_context,
            call->work,
            state,
            hoplite_cosocket_pool_active_cleanup) != HOPLITE_HOST_RESOURCE_OK)
    {
        ngx_free(state->key);
        ngx_free(state);
        return NGX_ERROR;
    }
    state->next = hoplite_cosocket_pool_active;
    state->linked = 1;
    hoplite_cosocket_pool_active = state;
    return NGX_OK;
}

static void
hoplite_cosocket_pool_idle_unlink(hoplite_cosocket_pool_idle_t *entry)
{
    hoplite_cosocket_pool_idle_t **link;

    if (entry == NULL || !entry->linked) {
        return;
    }
    for (link = &hoplite_cosocket_pool_idle;
         *link != NULL;
         link = &(*link)->next)
    {
        if (*link == entry) {
            *link = entry->next;
            entry->next = NULL;
            entry->linked = 0;
            return;
        }
    }
    entry->linked = 0;
}

static void
hoplite_cosocket_pool_idle_close(hoplite_cosocket_pool_idle_t *entry)
{
    ngx_connection_t *connection;

    if (entry == NULL) {
        return;
    }
    hoplite_cosocket_pool_idle_unlink(entry);
    connection = entry->connection;
    entry->connection = NULL;
    if (connection != NULL) {
        if (connection->read != NULL && connection->read->timer_set) {
            ngx_del_timer(connection->read);
        }
        if (connection->write != NULL && connection->write->timer_set) {
            ngx_del_timer(connection->write);
        }
        connection->data = NULL;
        ngx_close_connection(connection);
    }
    if (entry->pool != NULL) {
        ngx_destroy_pool(entry->pool);
        entry->pool = NULL;
    }
    if (entry->key != NULL) {
        ngx_free(entry->key);
        entry->key = NULL;
    }
    ngx_free(entry);
}

static ngx_flag_t
hoplite_cosocket_pool_connection_clean(ngx_connection_t *connection)
{
    u_char byte;
    ssize_t received;
    ngx_err_t error;

    if (connection == NULL
        || connection->fd == (ngx_socket_t) -1
        || connection->read == NULL
        || connection->write == NULL
        || connection->read->error
        || connection->read->eof
        || connection->read->timedout
        || connection->write->error
        || connection->write->timedout)
    {
        return 0;
    }

    received = recv(connection->fd, &byte, 1, MSG_PEEK);
    if (received == -1) {
        error = ngx_socket_errno;
        return error == NGX_EAGAIN;
    }
    return 0;
}

static void
hoplite_cosocket_pool_idle_handler(ngx_event_t *event)
{
    ngx_connection_t *connection = event == NULL ? NULL : event->data;
    hoplite_cosocket_pool_idle_t *entry =
        connection == NULL ? NULL : connection->data;

    if (entry != NULL && entry->connection == connection) {
        hoplite_cosocket_pool_idle_close(entry);
    }
}

static hoplite_cosocket_pool_idle_t *
hoplite_cosocket_pool_take(const u_char *key, size_t key_len)
{
    hoplite_cosocket_pool_idle_t *entry, *next;
    ngx_msec_t now = ngx_current_msec;

    for (entry = hoplite_cosocket_pool_idle;
         entry != NULL;
         entry = next)
    {
        next = entry->next;
        if (!hoplite_cosocket_pool_key_equal(
                entry->key, entry->key_len, key, key_len))
        {
            continue;
        }
        hoplite_cosocket_pool_idle_unlink(entry);
        if ((entry->expires != 0
             && (ngx_msec_int_t) (entry->expires - now) <= 0)
            || !hoplite_cosocket_pool_connection_clean(entry->connection))
        {
            hoplite_cosocket_pool_idle_close(entry);
            continue;
        }
        if (entry->connection->read->timer_set) {
            ngx_del_timer(entry->connection->read);
        }
        if (entry->connection->write->timer_set) {
            ngx_del_timer(entry->connection->write);
        }
        entry->connection->read->timedout = 0;
        entry->connection->write->timedout = 0;
        return entry;
    }
    return NULL;
}

static ngx_uint_t
hoplite_cosocket_pool_idle_count(const u_char *key, size_t key_len)
{
    hoplite_cosocket_pool_idle_t *entry;
    ngx_uint_t count = 0;

    for (entry = hoplite_cosocket_pool_idle;
         entry != NULL;
         entry = entry->next)
    {
        if (hoplite_cosocket_pool_key_equal(
                entry->key, entry->key_len, key, key_len))
        {
            count++;
        }
    }
    return count;
}

static hoplite_cosocket_pool_idle_t *
hoplite_cosocket_pool_oldest(const u_char *key, size_t key_len)
{
    hoplite_cosocket_pool_idle_t *entry, *oldest = NULL;

    for (entry = hoplite_cosocket_pool_idle;
         entry != NULL;
         entry = entry->next)
    {
        if (!hoplite_cosocket_pool_key_equal(
                entry->key, entry->key_len, key, key_len))
        {
            continue;
        }
        if (oldest == NULL || entry->serial < oldest->serial) {
            oldest = entry;
        }
    }
    return oldest;
}

static void
hoplite_cosocket_pool_make_capacity(const u_char *key,
                                    size_t key_len,
                                    ngx_uint_t size)
{
    hoplite_cosocket_pool_idle_t *oldest;

    while (hoplite_cosocket_pool_idle_count(key, key_len) >= size) {
        oldest = hoplite_cosocket_pool_oldest(key, key_len);
        if (oldest == NULL) {
            return;
        }
        hoplite_cosocket_pool_idle_close(oldest);
    }
}

static ngx_int_t
hoplite_cosocket_pool_options(const hoplite_hta_value_t *options,
                              ngx_str_t *name,
                              ngx_uint_t *size,
                              const char **error)
{
    hoplite_hta_value_t *name_value, *size_value, *backlog_value;
    int64_t number;
    size_t recognized = 0, index;

    if (options == NULL
        || (options->kind != HOPLITE_HTA_MAP
            && options->kind != HOPLITE_HTA_OBJECT))
    {
        *error = "cosocket connect options must be a map";
        return NGX_ERROR;
    }
    ngx_memzero(name, sizeof(*name));
    *size = HOPLITE_COSOCKET_POOL_DEFAULT_SIZE;

    name_value = hoplite_hta_map_get(options, "pool");
    if (name_value != NULL) {
        recognized++;
        if (hoplite_hta_text(name_value, name) != NGX_OK
            || name->len == 0 || name->len > 255)
        {
            *error = "cosocket :pool must contain 1..255 bytes";
            return NGX_ERROR;
        }
        for (index = 0; index < name->len; index++) {
            if (name->data[index] == '\0') {
                *error = "cosocket :pool cannot contain a NUL byte";
                return NGX_ERROR;
            }
        }
    }

    size_value = hoplite_hta_map_get(options, "pool-size");
    if (size_value != NULL) {
        recognized++;
        if (hoplite_hta_number(size_value, &number) != NGX_OK
            || number < 1 || number > HOPLITE_COSOCKET_POOL_MAX_SIZE)
        {
            *error = "cosocket :pool-size must be between 1 and 1024";
            return NGX_ERROR;
        }
        *size = (ngx_uint_t) number;
    }

    backlog_value = hoplite_hta_map_get(options, "backlog");
    if (backlog_value != NULL) {
        recognized++;
        if (hoplite_hta_number(backlog_value, &number) != NGX_OK
            || number != 0)
        {
            *error = "positive cosocket :backlog lands in the next pool slice";
            return NGX_ERROR;
        }
    }

    if (recognized != options->as.map.count) {
        *error = "cosocket connect supports only :pool, :pool-size, and zero :backlog";
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_pool_host_spec(ngx_pool_t *pool,
                                const hoplite_hta_value_t *arguments,
                                hoplite_cosocket_pool_spec_t *spec,
                                const char **error)
{
    hoplite_hta_value_t *options;
    ngx_addr_t numeric;
    ngx_str_t host, name;
    u_char *cursor, *end;
    int64_t port;
    size_t index, capacity;
    const char *prefix;
    size_t prefix_len;

    if (arguments == NULL || arguments->as.vector.count != 4
        || hoplite_hta_text(arguments->as.vector.items[1], &host) != NGX_OK
        || host.len == 0 || host.len > 255
        || hoplite_hta_number(arguments->as.vector.items[2], &port) != NGX_OK
        || port < 1 || port > 65535)
    {
        *error = "hoplite.socket/connect requires a host and port 1..65535";
        return NGX_ERROR;
    }
    options = arguments->as.vector.items[3];
    if (hoplite_cosocket_pool_options(
            options, &name, &spec->size, error) != NGX_OK)
    {
        return NGX_ERROR;
    }

    spec->numeric = ngx_parse_addr(pool, &numeric, host.data, host.len) == NGX_OK;
    if (!spec->numeric) {
        for (index = 0; index < host.len; index++) {
            if (host.data[index] == '\0'
                || host.data[index] == '/'
                || host.data[index] == ':'
                || host.data[index] == ' '
                || host.data[index] == '\t'
                || host.data[index] == '\r'
                || host.data[index] == '\n')
            {
                *error = "hostname contains an unsupported character";
                return NGX_ERROR;
            }
        }
    }

    prefix = spec->numeric ? "tcp:numeric:" : "tcp:hostname:";
    prefix_len = ngx_strlen(prefix);
    capacity = prefix_len + host.len + name.len + 48;
    if (capacity > HOPLITE_COSOCKET_POOL_MAX_KEY) {
        *error = "cosocket pool identity is too long";
        return NGX_ERROR;
    }
    spec->key.data = ngx_pnalloc(pool, capacity);
    if (spec->key.data == NULL) {
        return NGX_ERROR;
    }
    cursor = ngx_cpymem(spec->key.data, prefix, prefix_len);
    end = spec->key.data + capacity;
    for (index = 0; index < host.len; index++) {
        *cursor++ = spec->numeric ? host.data[index] : ngx_tolower(host.data[index]);
    }
    *cursor++ = ':';
    cursor = ngx_snprintf(cursor, (size_t) (end - cursor), "%L", port);
    if (name.len != 0) {
        cursor = ngx_cpymem(cursor, "|pool:", sizeof("|pool:") - 1);
        cursor = ngx_cpymem(cursor, name.data, name.len);
    }
    spec->key.len = (size_t) (cursor - spec->key.data);
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_pool_unix_spec(ngx_pool_t *pool,
                                const hoplite_hta_value_t *arguments,
                                hoplite_cosocket_pool_spec_t *spec,
                                const char **error)
{
    ngx_str_t target;
    size_t capacity;
    u_char *cursor;

    if (arguments == NULL || arguments->as.vector.count != 2
        || hoplite_hta_text(arguments->as.vector.items[1], &target) != NGX_OK
        || target.len < sizeof("unix:/") - 1
        || ngx_strncmp(target.data,
                       "unix:/",
                       sizeof("unix:/") - 1) != 0)
    {
        *error = "Unix-domain target must be unix:/absolute/path";
        return NGX_ERROR;
    }
    capacity = sizeof("unix:") - 1 + target.len;
    if (capacity > HOPLITE_COSOCKET_POOL_MAX_KEY) {
        *error = "cosocket pool identity is too long";
        return NGX_ERROR;
    }
    spec->key.data = ngx_pnalloc(pool, capacity);
    if (spec->key.data == NULL) {
        return NGX_ERROR;
    }
    cursor = ngx_cpymem(spec->key.data,
                        "unix:",
                        sizeof("unix:") - 1);
    cursor = ngx_cpymem(cursor, target.data, target.len);
    spec->key.len = (size_t) (cursor - spec->key.data);
    spec->size = HOPLITE_COSOCKET_POOL_DEFAULT_SIZE;
    spec->numeric = 0;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_pool_checkout(const hoplite_host_call_v1_t *call,
                                hoplite_cosocket_t *socket,
                                hoplite_cosocket_pool_idle_t *entry,
                                ngx_uint_t size)
{
    ngx_connection_t *connection;
    ngx_pool_t *fresh_pool;
    uint64_t reused;

    reused = entry->reused == INT64_MAX
        ? (uint64_t) INT64_MAX : entry->reused + 1;
    if (hoplite_cosocket_pool_active_add(call,
                                         socket,
                                         entry->key,
                                         entry->key_len,
                                         size,
                                         reused) != NGX_OK)
    {
        hoplite_cosocket_pool_idle_close(entry);
        return hoplite_cosocket_reject(
            call, "could not register pooled cosocket cleanup");
    }

    connection = entry->connection;
    fresh_pool = socket->pool;
    socket->pool = entry->pool;
    entry->connection = NULL;
    entry->pool = NULL;
    if (entry->key != NULL) {
        ngx_free(entry->key);
    }
    ngx_free(entry);
    if (fresh_pool != NULL) {
        ngx_destroy_pool(fresh_pool);
    }

    ngx_memzero(&socket->peer, sizeof(socket->peer));
    socket->connection = connection;
    socket->peer.connection = connection;
    socket->peer.data = socket;
    socket->peer.log = socket->log;
    socket->connected = 1;
    socket->closed = 0;
    socket->pending = HOPLITE_COSOCKET_PENDING_NONE;
    connection->data = socket;
    connection->idle = 0;
    connection->read->handler = hoplite_cosocket_read_handler;
    connection->write->handler = hoplite_cosocket_write_handler;
    connection->read->log = socket->log;
    connection->write->log = socket->log;
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_pool_connect(const hoplite_host_call_v1_t *call,
                              hoplite_hta_value_t *arguments,
                              ngx_pool_t *pool)
{
    hoplite_cosocket_pool_spec_t spec;
    hoplite_cosocket_pool_idle_t *entry;
    hoplite_cosocket_t *socket;
    hoplite_hta_value_t *options;
    const char *error = NULL;
    ngx_int_t rc;

    ngx_memzero(&spec, sizeof(spec));
    if (arguments->as.vector.count == 2) {
        rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
        if (rc == NGX_ERROR) {
            return hoplite_cosocket_reject(
                call, "hoplite.socket/connect expects [socket unix-target]");
        }
        if (rc == NGX_DECLINED) {
            return hoplite_cosocket_reject(
                call, "unknown or foreign Hoplite cosocket");
        }
        if (hoplite_cosocket_pool_unix_spec(
                pool, arguments, &spec, &error) != NGX_OK)
        {
            return error == NULL
                ? HOPLITE_HOST_PROVIDER_ERROR
                : hoplite_cosocket_reject(call, error);
        }
    } else if (arguments->as.vector.count == 4) {
        rc = hoplite_cosocket_argument_handle(call, arguments, 4, &socket);
        if (rc == NGX_ERROR) {
            return hoplite_cosocket_reject(
                call, "hoplite.socket/connect expects [socket host port options]");
        }
        if (rc == NGX_DECLINED) {
            return hoplite_cosocket_reject(
                call, "unknown or foreign Hoplite cosocket");
        }
        if (hoplite_cosocket_pool_host_spec(
                pool, arguments, &spec, &error) != NGX_OK)
        {
            return error == NULL
                ? HOPLITE_HOST_PROVIDER_ERROR
                : hoplite_cosocket_reject(call, error);
        }
    } else {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/connect expects a TCP or Unix target");
    }

    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (socket->connected || socket->connection != NULL) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "already connected");
    }

    entry = hoplite_cosocket_pool_take(spec.key.data, spec.key.len);
    if (entry != NULL) {
        return hoplite_cosocket_pool_checkout(call, socket, entry, spec.size);
    }
    if (hoplite_cosocket_pool_active_add(call,
                                         socket,
                                         spec.key.data,
                                         spec.key.len,
                                         spec.size,
                                         0) != NGX_OK)
    {
        return hoplite_cosocket_reject(
            call, "could not register cosocket pool identity");
    }

    if (arguments->as.vector.count == 2) {
        return hoplite_cosocket_connect_unix(call, arguments);
    }
    options = arguments->as.vector.items[3];
    options->as.map.count = 0;
    return spec.numeric
        ? hoplite_cosocket_connect(call, arguments)
        : hoplite_cosocket_connect_hostname(call, arguments, pool);
}

static ngx_int_t
hoplite_cosocket_pool_setkeepalive(const hoplite_host_call_v1_t *call,
                                   const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_pool_active_t *active;
    hoplite_cosocket_pool_idle_t *entry;
    hoplite_cosocket_t *socket;
    ngx_connection_t *connection;
    ngx_pool_t *replacement;
    ngx_msec_t timeout = HOPLITE_COSOCKET_POOL_DEFAULT_TIMEOUT;
    ngx_uint_t size;
    int64_t handle, number;
    size_t count;

    count = arguments == NULL ? 0 : arguments->as.vector.count;
    if (count < 1 || count > 3
        || hoplite_hta_number(arguments->as.vector.items[0], &handle) != NGX_OK
        || handle <= 0)
    {
        return hoplite_cosocket_reject(
            call,
            "hoplite.socket/setkeepalive expects [socket timeout pool-size]");
    }
    socket = hoplite_cosocket_find(call->request_context,
                                   call->work,
                                   (uint64_t) handle);
    if (socket == NULL) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    active = hoplite_cosocket_pool_active_find(socket);
    size = active == NULL
        ? HOPLITE_COSOCKET_POOL_DEFAULT_SIZE : active->size;
    if (count >= 2
        && (hoplite_hta_number(arguments->as.vector.items[1], &number) != NGX_OK
            || number < 0 || number > HOPLITE_COSOCKET_MAX_TIMEOUT))
    {
        return hoplite_cosocket_reject(
            call, "keepalive timeout must be between 0 and 3600000 ms");
    }
    if (count >= 2) {
        timeout = (ngx_msec_t) number;
    }
    if (count == 3
        && (hoplite_hta_number(arguments->as.vector.items[2], &number) != NGX_OK
            || number < 1 || number > HOPLITE_COSOCKET_POOL_MAX_SIZE))
    {
        return hoplite_cosocket_reject(
            call, "keepalive pool size must be between 1 and 1024");
    }
    if (count == 3) {
        size = (ngx_uint_t) number;
    }

    if (socket->closed || !socket->connected
        || socket->connection == NULL)
    {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (active == NULL || active->key == NULL) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "connection has no pool identity");
    }
    if (!active->poolable) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "socket options prevent safe pooling");
    }
    if (socket->pending != HOPLITE_COSOCKET_PENDING_NONE
        || socket->leftover_len != 0
        || socket->receive_len != 0
        || socket->send_offset != socket->send_len
        || hoplite_cosocket_send_is_shutdown(socket)
        || !hoplite_cosocket_pool_connection_clean(socket->connection))
    {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "connection in dubious state");
    }

    replacement = ngx_create_pool(4096, socket->log);
    entry = ngx_alloc(sizeof(*entry), socket->log);
    if (replacement == NULL || entry == NULL) {
        if (replacement != NULL) {
            ngx_destroy_pool(replacement);
        }
        if (entry != NULL) {
            ngx_free(entry);
        }
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ngx_memzero(entry, sizeof(*entry));
    connection = socket->connection;
    hoplite_cosocket_clear_timer(socket);
    hoplite_cosocket_clear_io(socket);
    hoplite_cosocket_clear_leftover(socket);

    entry->connection = connection;
    entry->pool = socket->pool;
    entry->key = active->key;
    entry->key_len = active->key_len;
    entry->reused = active->reused;
    entry->serial = ++hoplite_cosocket_pool_serial;
    entry->expires = timeout == 0 ? 0 : ngx_current_msec + timeout;
    active->key = NULL;
    active->key_len = 0;
    hoplite_cosocket_pool_active_retire(active, 1);

    socket->pool = replacement;
    socket->connection = NULL;
    ngx_memzero(&socket->peer, sizeof(socket->peer));
    socket->connected = 0;
    socket->closed = 1;
    socket->readers = NULL;

    connection->data = entry;
    connection->idle = 1;
    connection->read->handler = hoplite_cosocket_pool_idle_handler;
    connection->write->handler = hoplite_cosocket_pool_idle_handler;
    connection->read->log = socket->log;
    connection->write->log = socket->log;

    hoplite_cosocket_pool_make_capacity(entry->key, entry->key_len, size);
    entry->next = hoplite_cosocket_pool_idle;
    entry->linked = 1;
    hoplite_cosocket_pool_idle = entry;
    if (timeout != 0) {
        ngx_add_timer(connection->read, timeout);
    }
    if (ngx_handle_read_event(connection->read, 0) != NGX_OK) {
        hoplite_cosocket_pool_idle_close(entry);
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "could not monitor idle connection");
    }
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_pool_getreusedtimes(const hoplite_host_call_v1_t *call,
                                     const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_pool_active_t *active;
    hoplite_cosocket_t *socket;
    ngx_int_t rc = hoplite_cosocket_argument_handle(
        call, arguments, 1, &socket);

    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/getreusedtimes expects [socket]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(
            call, "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    active = hoplite_cosocket_pool_active_find(socket);
    return hoplite_cosocket_complete_ordinary_call(
        call,
        1,
        active == NULL ? 0 : (int64_t) active->reused,
        NULL);
}

static void
hoplite_cosocket_pool_mark_optioned(const hoplite_host_call_v1_t *call,
                                    const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_pool_active_t *active;
    hoplite_cosocket_t *socket;

    if (hoplite_cosocket_argument_handle(call, arguments, 3, &socket) != NGX_OK) {
        return;
    }
    active = hoplite_cosocket_pool_active_find(socket);
    if (active != NULL) {
        active->poolable = 0;
    }
}

static void
hoplite_cosocket_pool_init(void)
{
    hoplite_cosocket_pool_active = NULL;
    hoplite_cosocket_pool_idle = NULL;
    hoplite_cosocket_pool_serial = 0;
}

static void
hoplite_cosocket_pool_worker_exit(void)
{
    hoplite_cosocket_pool_active_t *active;
    hoplite_cosocket_pool_idle_t *idle;

    while (hoplite_cosocket_pool_idle != NULL) {
        idle = hoplite_cosocket_pool_idle;
        hoplite_cosocket_pool_idle_close(idle);
    }
    for (active = hoplite_cosocket_pool_active;
         active != NULL;
         active = active->next)
    {
        active->linked = 0;
        active->socket = NULL;
        if (active->key != NULL) {
            ngx_free(active->key);
            active->key = NULL;
        }
    }
    hoplite_cosocket_pool_active = NULL;
    hoplite_cosocket_pool_serial = 0;
}

#endif
