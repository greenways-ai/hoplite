#include "hoplite_cosocket.h"

#include <ngx_event_connect.h>

#include "hoplite_host_provider.h"
#include "hoplite_hta.h"

#define HOPLITE_COSOCKET_DEFAULT_TIMEOUT 60000
#define HOPLITE_COSOCKET_MAX_TIMEOUT 3600000
#define HOPLITE_COSOCKET_MAX_IO (1024u * 1024u)
#define HOPLITE_COSOCKET_READ_CHUNK 4096u

#define HOPLITE_HTA_NIL 0u
#define HOPLITE_HTA_I64 3u
#define HOPLITE_HTA_STRING 4u
#define HOPLITE_HTA_BYTES 5u
#define HOPLITE_HTA_VECTOR 9u

typedef enum {
    HOPLITE_COSOCKET_PENDING_NONE = 0,
    HOPLITE_COSOCKET_PENDING_CONNECT,
    HOPLITE_COSOCKET_PENDING_SEND,
    HOPLITE_COSOCKET_PENDING_RECEIVE
} hoplite_cosocket_pending_t;

typedef enum {
    HOPLITE_COSOCKET_RECEIVE_FIXED = 0,
    HOPLITE_COSOCKET_RECEIVE_LINE,
    HOPLITE_COSOCKET_RECEIVE_ALL,
    HOPLITE_COSOCKET_RECEIVE_ANY
} hoplite_cosocket_receive_mode_t;

typedef struct hoplite_cosocket_s hoplite_cosocket_t;

struct hoplite_cosocket_s {
    uint64_t id;
    void *owner;
    uint64_t work;
    ngx_pool_t *pool;
    ngx_log_t *log;
    ngx_peer_connection_t peer;
    ngx_connection_t *connection;
    ngx_msec_t connect_timeout;
    ngx_msec_t send_timeout;
    ngx_msec_t read_timeout;
    ngx_flag_t connected;
    ngx_flag_t closed;
    ngx_flag_t released;
    hoplite_cosocket_pending_t pending;
    uint64_t pending_call;
    hoplite_host_completer_v1_t completer;
    u_char *send_data;
    size_t send_len;
    size_t send_offset;
    u_char *receive_data;
    size_t receive_len;
    size_t receive_capacity;
    size_t receive_target;
    hoplite_cosocket_receive_mode_t receive_mode;
    u_char *leftover;
    size_t leftover_len;
    size_t leftover_capacity;
    hoplite_cosocket_t *next;
};

typedef struct {
    u_char *data;
    size_t len;
    size_t cursor;
} hoplite_cosocket_writer_t;

static const u_char hoplite_cosocket_hta_magic[] = {HOPLITE_HTA_MAGIC_BYTES};
static hoplite_cosocket_t *hoplite_cosockets;
static uint64_t hoplite_cosocket_next_id;
static ngx_log_t *hoplite_cosocket_log;

static int32_t hoplite_cosocket_invoke(const hoplite_host_call_v1_t *call);
static void hoplite_cosocket_cancel(void *request_context);
static void hoplite_cosocket_request_cleanup(void *data);
static void hoplite_cosocket_read_handler(ngx_event_t *event);
static void hoplite_cosocket_write_handler(ngx_event_t *event);

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

static ngx_int_t
hoplite_cosocket_write(hoplite_cosocket_writer_t *writer,
                       const void *data,
                       size_t len)
{
    if (writer == NULL || len > writer->len
        || writer->cursor > writer->len - len)
    {
        return NGX_ERROR;
    }
    if (len != 0) {
        ngx_memcpy(writer->data + writer->cursor, data, len);
    }
    writer->cursor += len;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_write_byte(hoplite_cosocket_writer_t *writer, u_char value)
{
    return hoplite_cosocket_write(writer, &value, 1);
}

static ngx_int_t
hoplite_cosocket_write_u32(hoplite_cosocket_writer_t *writer, uint32_t value)
{
    u_char data[4];
    data[0] = (u_char) (value >> 24);
    data[1] = (u_char) (value >> 16);
    data[2] = (u_char) (value >> 8);
    data[3] = (u_char) value;
    return hoplite_cosocket_write(writer, data, sizeof(data));
}

static ngx_int_t
hoplite_cosocket_write_i64(hoplite_cosocket_writer_t *writer, int64_t value)
{
    uint64_t raw = (uint64_t) value;
    u_char data[8] = {
        (u_char) (raw >> 56), (u_char) (raw >> 48),
        (u_char) (raw >> 40), (u_char) (raw >> 32),
        (u_char) (raw >> 24), (u_char) (raw >> 16),
        (u_char) (raw >> 8), (u_char) raw
    };
    return hoplite_cosocket_write(writer, data, sizeof(data));
}

static ngx_int_t
hoplite_cosocket_write_nil(hoplite_cosocket_writer_t *writer)
{
    return hoplite_cosocket_write_byte(writer, HOPLITE_HTA_NIL);
}

static ngx_int_t
hoplite_cosocket_write_number(hoplite_cosocket_writer_t *writer, int64_t value)
{
    return hoplite_cosocket_write_byte(writer, HOPLITE_HTA_I64) == NGX_OK
        && hoplite_cosocket_write_i64(writer, value) == NGX_OK
        ? NGX_OK : NGX_ERROR;
}

static ngx_int_t
hoplite_cosocket_write_text(hoplite_cosocket_writer_t *writer,
                            u_char tag,
                            const u_char *data,
                            size_t len)
{
    if (len > UINT32_MAX
        || hoplite_cosocket_write_byte(writer, tag) != NGX_OK
        || hoplite_cosocket_write_u32(writer, (uint32_t) len) != NGX_OK)
    {
        return NGX_ERROR;
    }
    return hoplite_cosocket_write(writer, data, len);
}

static ngx_int_t
hoplite_cosocket_writer_init(hoplite_cosocket_writer_t *writer,
                             size_t capacity,
                             ngx_log_t *log)
{
    if (writer == NULL || capacity < sizeof(hoplite_cosocket_hta_magic)) {
        return NGX_ERROR;
    }
    writer->data = ngx_alloc(capacity, log);
    if (writer->data == NULL) {
        return NGX_ERROR;
    }
    writer->len = capacity;
    writer->cursor = 0;
    if (hoplite_cosocket_write(writer,
                               hoplite_cosocket_hta_magic,
                               sizeof(hoplite_cosocket_hta_magic)) != NGX_OK)
    {
        ngx_free(writer->data);
        writer->data = NULL;
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_deliver(const hoplite_host_completer_v1_t *completer,
                         ngx_log_t *log,
                         ngx_flag_t failure,
                         hoplite_cosocket_writer_t *writer)
{
    int32_t rc;

    if (completer == NULL || writer == NULL || writer->data == NULL
        || completer->succeed == NULL || completer->fail == NULL)
    {
        return NGX_ERROR;
    }
    rc = failure
        ? completer->fail(completer->context, writer->data, writer->cursor)
        : completer->succeed(completer->context, writer->data, writer->cursor);
    ngx_free(writer->data);
    writer->data = NULL;
    if (rc != HOPLITE_HOST_PROVIDER_OK) {
        ngx_log_error(NGX_LOG_ERR, log, 0,
                      "hoplite cosocket result delivery failed");
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_complete_number(const hoplite_host_call_v1_t *call,
                                 int64_t value)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};

    if (hoplite_cosocket_writer_init(&writer, 32, hoplite_cosocket_log) != NGX_OK
        || hoplite_cosocket_write_number(&writer, value) != NGX_OK)
    {
        if (writer.data != NULL) {
            ngx_free(writer.data);
        }
        return NGX_ERROR;
    }
    return hoplite_cosocket_deliver(&call->completer,
                                    hoplite_cosocket_log,
                                    0,
                                    &writer);
}

static ngx_int_t
hoplite_cosocket_reject(const hoplite_host_call_v1_t *call,
                        const char *message)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};
    size_t len = ngx_strlen(message);

    if (len > (size_t) -1 - 32
        || hoplite_cosocket_writer_init(&writer, len + 32,
                                        hoplite_cosocket_log) != NGX_OK
        || hoplite_cosocket_write_text(&writer,
                                       HOPLITE_HTA_STRING,
                                       (const u_char *) message,
                                       len) != NGX_OK)
    {
        if (writer.data != NULL) {
            ngx_free(writer.data);
        }
        return NGX_ERROR;
    }
    return hoplite_cosocket_deliver(&call->completer,
                                    hoplite_cosocket_log,
                                    1,
                                    &writer);
}

static ngx_int_t
hoplite_cosocket_complete_ordinary_call(const hoplite_host_call_v1_t *call,
                                        ngx_flag_t success,
                                        int64_t value,
                                        const char *error)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};
    size_t error_len = error == NULL ? 0 : ngx_strlen(error);

    if (error_len > (size_t) -1 - 64
        || hoplite_cosocket_writer_init(&writer, error_len + 64,
                                        hoplite_cosocket_log) != NGX_OK
        || hoplite_cosocket_write_byte(&writer, HOPLITE_HTA_VECTOR) != NGX_OK
        || hoplite_cosocket_write_u32(&writer, 2) != NGX_OK
        || (success
            ? hoplite_cosocket_write_number(&writer, value)
            : hoplite_cosocket_write_nil(&writer)) != NGX_OK
        || (success
            ? hoplite_cosocket_write_nil(&writer)
            : hoplite_cosocket_write_text(&writer,
                                           HOPLITE_HTA_STRING,
                                           (const u_char *) error,
                                           error_len)) != NGX_OK)
    {
        if (writer.data != NULL) {
            ngx_free(writer.data);
        }
        return NGX_ERROR;
    }
    return hoplite_cosocket_deliver(&call->completer,
                                    hoplite_cosocket_log,
                                    0,
                                    &writer);
}

static void
hoplite_cosocket_clear_timer(hoplite_cosocket_t *socket)
{
    ngx_event_t *event = NULL;

    if (socket == NULL || socket->connection == NULL) {
        return;
    }
    if (socket->pending == HOPLITE_COSOCKET_PENDING_RECEIVE) {
        event = socket->connection->read;
    } else if (socket->pending == HOPLITE_COSOCKET_PENDING_CONNECT
               || socket->pending == HOPLITE_COSOCKET_PENDING_SEND)
    {
        event = socket->connection->write;
    }
    if (event != NULL) {
        if (event->timer_set) {
            ngx_del_timer(event);
        }
        event->timedout = 0;
    }
}

static void
hoplite_cosocket_clear_io(hoplite_cosocket_t *socket)
{
    if (socket->send_data != NULL) {
        ngx_free(socket->send_data);
        socket->send_data = NULL;
    }
    if (socket->receive_data != NULL) {
        ngx_free(socket->receive_data);
        socket->receive_data = NULL;
    }
    socket->send_len = 0;
    socket->send_offset = 0;
    socket->receive_len = 0;
    socket->receive_capacity = 0;
    socket->receive_target = 0;
}

static void
hoplite_cosocket_clear_leftover(hoplite_cosocket_t *socket)
{
    if (socket->leftover != NULL) {
        ngx_free(socket->leftover);
        socket->leftover = NULL;
    }
    socket->leftover_len = 0;
    socket->leftover_capacity = 0;
}

static void
hoplite_cosocket_reset_connection(hoplite_cosocket_t *socket)
{
    if (socket == NULL) {
        return;
    }
    if (socket->connection != NULL) {
        ngx_close_connection(socket->connection);
        socket->connection = NULL;
        socket->peer.connection = NULL;
    }
    socket->connected = 0;
    hoplite_cosocket_clear_leftover(socket);
}

static void
hoplite_cosocket_close_state(hoplite_cosocket_t *socket)
{
    if (socket == NULL) {
        return;
    }
    hoplite_cosocket_clear_timer(socket);
    hoplite_cosocket_clear_io(socket);
    ngx_memzero(&socket->completer, sizeof(socket->completer));
    socket->pending = HOPLITE_COSOCKET_PENDING_NONE;
    socket->pending_call = 0;
    hoplite_cosocket_reset_connection(socket);
    socket->closed = 1;
}

static void
hoplite_cosocket_unlink(hoplite_cosocket_t *socket)
{
    hoplite_cosocket_t **link;

    for (link = &hoplite_cosockets; *link != NULL; link = &(*link)->next) {
        if (*link == socket) {
            *link = socket->next;
            socket->next = NULL;
            return;
        }
    }
}

static void
hoplite_cosocket_request_cleanup(void *data)
{
    hoplite_cosocket_t *socket = data;

    if (socket == NULL || socket->released) {
        return;
    }
    socket->released = 1;
    hoplite_cosocket_unlink(socket);
    hoplite_cosocket_close_state(socket);
    if (socket->pool != NULL) {
        ngx_destroy_pool(socket->pool);
        socket->pool = NULL;
    }
    ngx_free(socket);
}

static hoplite_cosocket_t *
hoplite_cosocket_find(void *owner, uint64_t work, uint64_t id)
{
    hoplite_cosocket_t *socket;

    for (socket = hoplite_cosockets;
         socket != NULL;
         socket = socket->next)
    {
        if (!socket->released
            && socket->id == id
            && socket->owner == owner
            && socket->work == work)
        {
            return socket;
        }
    }
    return NULL;
}

static ngx_int_t
hoplite_cosocket_result_writer(hoplite_cosocket_writer_t *writer,
                               size_t extra,
                               ngx_log_t *log)
{
    if (extra > (size_t) -1 - 96
        || hoplite_cosocket_writer_init(writer, extra + 96, log) != NGX_OK
        || hoplite_cosocket_write_byte(writer, HOPLITE_HTA_VECTOR) != NGX_OK)
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
hoplite_cosocket_deliver_state(hoplite_cosocket_t *socket,
                               hoplite_cosocket_writer_t *writer)
{
    hoplite_host_completer_v1_t completer;
    ngx_log_t *log;

    if (socket == NULL || writer == NULL) {
        return NGX_ERROR;
    }
    hoplite_cosocket_clear_timer(socket);
    hoplite_cosocket_clear_io(socket);
    completer = socket->completer;
    log = socket->log;
    ngx_memzero(&socket->completer, sizeof(socket->completer));
    socket->pending = HOPLITE_COSOCKET_PENDING_NONE;
    socket->pending_call = 0;
    return hoplite_cosocket_deliver(&completer, log, 0, writer);
}

static ngx_int_t
hoplite_cosocket_complete_ordinary(hoplite_cosocket_t *socket,
                                   ngx_flag_t success,
                                   int64_t value,
                                   const char *error)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};
    size_t error_len = error == NULL ? 0 : ngx_strlen(error);

    if (hoplite_cosocket_result_writer(&writer, error_len, socket->log) != NGX_OK
        || hoplite_cosocket_write_u32(&writer, 2) != NGX_OK
        || (success
            ? hoplite_cosocket_write_number(&writer, value)
            : hoplite_cosocket_write_nil(&writer)) != NGX_OK
        || (success
            ? hoplite_cosocket_write_nil(&writer)
            : hoplite_cosocket_write_text(&writer,
                                           HOPLITE_HTA_STRING,
                                           (const u_char *) error,
                                           error_len)) != NGX_OK)
    {
        if (writer.data != NULL) {
            ngx_free(writer.data);
        }
        return NGX_ERROR;
    }
    return hoplite_cosocket_deliver_state(socket, &writer);
}

static ngx_int_t
hoplite_cosocket_complete_receive(hoplite_cosocket_t *socket,
                                  ngx_flag_t success,
                                  const char *error)
{
    hoplite_cosocket_writer_t writer = {NULL, 0, 0};
    size_t error_len = error == NULL ? 0 : ngx_strlen(error);
    size_t data_len = socket->receive_len;
    u_char *data = socket->receive_data;

    if (data_len > (size_t) -1 - error_len
        || hoplite_cosocket_result_writer(&writer,
                                          data_len + error_len,
                                          socket->log) != NGX_OK
        || hoplite_cosocket_write_u32(&writer, 3) != NGX_OK
        || (success
            ? hoplite_cosocket_write_text(&writer,
                                           HOPLITE_HTA_BYTES,
                                           data,
                                           data_len)
            : hoplite_cosocket_write_nil(&writer)) != NGX_OK
        || (success
            ? hoplite_cosocket_write_nil(&writer)
            : hoplite_cosocket_write_text(&writer,
                                           HOPLITE_HTA_STRING,
                                           (const u_char *) error,
                                           error_len)) != NGX_OK
        || (success
            ? hoplite_cosocket_write_nil(&writer)
            : hoplite_cosocket_write_text(&writer,
                                           HOPLITE_HTA_BYTES,
                                           data,
                                           data_len)) != NGX_OK)
    {
        if (writer.data != NULL) {
            ngx_free(writer.data);
        }
        return NGX_ERROR;
    }
    return hoplite_cosocket_deliver_state(socket, &writer);
}

static const char *
hoplite_cosocket_error_text(ngx_err_t error, const char *fallback)
{
    if (error == NGX_ETIMEDOUT) {
        return "timeout";
    }
#ifdef NGX_ECONNREFUSED
    if (error == NGX_ECONNREFUSED) {
        return "connection refused";
    }
#endif
#ifdef NGX_ECONNRESET
    if (error == NGX_ECONNRESET) {
        return "connection reset by peer";
    }
#endif
#ifdef NGX_EPIPE
    if (error == NGX_EPIPE) {
        return "closed";
    }
#endif
#ifdef NGX_ENETUNREACH
    if (error == NGX_ENETUNREACH) {
        return "network unreachable";
    }
#endif
#ifdef NGX_EHOSTUNREACH
    if (error == NGX_EHOSTUNREACH) {
        return "host unreachable";
    }
#endif
    return fallback;
}

static ngx_int_t
hoplite_cosocket_grow(u_char **buffer,
                      size_t *capacity,
                      size_t required,
                      ngx_log_t *log)
{
    u_char *replacement;
    size_t next;

    if (required > HOPLITE_COSOCKET_MAX_IO) {
        return NGX_ERROR;
    }
    if (*capacity >= required) {
        return NGX_OK;
    }
    next = *capacity == 0 ? HOPLITE_COSOCKET_READ_CHUNK : *capacity;
    while (next < required) {
        if (next > HOPLITE_COSOCKET_MAX_IO / 2) {
            next = HOPLITE_COSOCKET_MAX_IO;
            break;
        }
        next *= 2;
    }
    replacement = ngx_alloc(next, log);
    if (replacement == NULL) {
        return NGX_ERROR;
    }
    if (*buffer != NULL && *capacity != 0) {
        ngx_memcpy(replacement, *buffer, *capacity);
        ngx_free(*buffer);
    }
    *buffer = replacement;
    *capacity = next;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_receive_append(hoplite_cosocket_t *socket,
                                const u_char *data,
                                size_t len)
{
    if (len > HOPLITE_COSOCKET_MAX_IO - socket->receive_len
        || hoplite_cosocket_grow(&socket->receive_data,
                                 &socket->receive_capacity,
                                 socket->receive_len + len,
                                 socket->log) != NGX_OK)
    {
        return NGX_ERROR;
    }
    if (len != 0) {
        ngx_memcpy(socket->receive_data + socket->receive_len, data, len);
    }
    socket->receive_len += len;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_leftover_set(hoplite_cosocket_t *socket,
                              const u_char *data,
                              size_t len)
{
    if (len == 0) {
        socket->leftover_len = 0;
        return NGX_OK;
    }
    if (hoplite_cosocket_grow(&socket->leftover,
                              &socket->leftover_capacity,
                              len,
                              socket->log) != NGX_OK)
    {
        return NGX_ERROR;
    }
    ngx_memcpy(socket->leftover, data, len);
    socket->leftover_len = len;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_receive_line_data(hoplite_cosocket_t *socket,
                                   const u_char *data,
                                   size_t len,
                                   ngx_flag_t *complete)
{
    size_t index;

    *complete = 0;
    for (index = 0; index < len; index++) {
        if (data[index] == '\n') {
            if (hoplite_cosocket_receive_append(socket, data, index) != NGX_OK
                || hoplite_cosocket_leftover_set(socket,
                                                 data + index + 1,
                                                 len - index - 1) != NGX_OK)
            {
                return NGX_ERROR;
            }
            if (socket->receive_len != 0
                && socket->receive_data[socket->receive_len - 1] == '\r')
            {
                socket->receive_len--;
            }
            *complete = 1;
            return NGX_OK;
        }
    }
    return hoplite_cosocket_receive_append(socket, data, len);
}

static ngx_int_t
hoplite_cosocket_arm_write(hoplite_cosocket_t *socket, ngx_msec_t timeout)
{
    ngx_event_t *event = socket->connection->write;

    if (timeout != 0 && !event->timer_set) {
        ngx_add_timer(event, timeout);
    }
    return ngx_handle_write_event(event, 0);
}

static ngx_int_t
hoplite_cosocket_arm_read(hoplite_cosocket_t *socket)
{
    ngx_event_t *event = socket->connection->read;

    if (socket->read_timeout != 0 && !event->timer_set) {
        ngx_add_timer(event, socket->read_timeout);
    }
    return ngx_handle_read_event(event, 0);
}

static ngx_int_t
hoplite_cosocket_connect_check(hoplite_cosocket_t *socket)
{
    int error = 0;
    socklen_t len = sizeof(error);

    if (getsockopt(socket->connection->fd,
                   SOL_SOCKET,
                   SO_ERROR,
                   (void *) &error,
                   &len) == -1)
    {
        error = ngx_socket_errno;
    }
    if (error != 0) {
        const char *message = hoplite_cosocket_error_text(
            (ngx_err_t) error, "connect failed");
        hoplite_cosocket_reset_connection(socket);
        return hoplite_cosocket_complete_ordinary(socket, 0, 0, message);
    }
    socket->connected = 1;
    return hoplite_cosocket_complete_ordinary(socket, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_send_drive(hoplite_cosocket_t *socket)
{
    ssize_t sent;

    while (socket->send_offset < socket->send_len) {
        sent = socket->connection->send(
            socket->connection,
            socket->send_data + socket->send_offset,
            socket->send_len - socket->send_offset);
        if (sent > 0) {
            socket->send_offset += (size_t) sent;
            continue;
        }
        if (sent == NGX_AGAIN) {
            if (hoplite_cosocket_arm_write(socket, socket->send_timeout)
                != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_ordinary(
                    socket, 0, 0, "send failed");
            }
            return NGX_AGAIN;
        }
        {
            const char *message = hoplite_cosocket_error_text(
                ngx_socket_errno, "send failed");
            hoplite_cosocket_reset_connection(socket);
            return hoplite_cosocket_complete_ordinary(
                socket, 0, 0, message);
        }
    }
    return hoplite_cosocket_complete_ordinary(
        socket, 1, (int64_t) socket->send_len, NULL);
}

static ngx_int_t
hoplite_cosocket_receive_drive(hoplite_cosocket_t *socket)
{
    u_char chunk[HOPLITE_COSOCKET_READ_CHUNK];
    size_t amount, line_index, suffix;
    ssize_t received;
    ngx_flag_t complete;

    if (socket->leftover_len != 0) {
        if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_ANY) {
            amount = socket->receive_target;
            if (amount > socket->leftover_len) {
                amount = socket->leftover_len;
            }
            if (hoplite_cosocket_receive_append(socket,
                                                 socket->leftover,
                                                 amount) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            if (amount < socket->leftover_len) {
                ngx_memmove(socket->leftover,
                            socket->leftover + amount,
                            socket->leftover_len - amount);
            }
            socket->leftover_len -= amount;
            return hoplite_cosocket_complete_receive(socket, 1, NULL);
        } else if (socket->receive_mode
                   == HOPLITE_COSOCKET_RECEIVE_FIXED)
        {
            amount = socket->receive_target - socket->receive_len;
            if (amount > socket->leftover_len) {
                amount = socket->leftover_len;
            }
            if (hoplite_cosocket_receive_append(socket,
                                                 socket->leftover,
                                                 amount) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            if (amount < socket->leftover_len) {
                ngx_memmove(socket->leftover,
                            socket->leftover + amount,
                            socket->leftover_len - amount);
            }
            socket->leftover_len -= amount;
            if (socket->receive_len == socket->receive_target) {
                return hoplite_cosocket_complete_receive(socket, 1, NULL);
            }
        } else if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_LINE) {
            complete = 0;
            for (line_index = 0;
                 line_index < socket->leftover_len;
                 line_index++)
            {
                if (socket->leftover[line_index] == '\n') {
                    if (hoplite_cosocket_receive_append(
                            socket, socket->leftover, line_index) != NGX_OK)
                    {
                        hoplite_cosocket_reset_connection(socket);
                        return hoplite_cosocket_complete_receive(
                            socket, 0, "buffer too small");
                    }
                    suffix = socket->leftover_len - line_index - 1;
                    if (suffix != 0) {
                        ngx_memmove(socket->leftover,
                                    socket->leftover + line_index + 1,
                                    suffix);
                    }
                    socket->leftover_len = suffix;
                    if (socket->receive_len != 0
                        && socket->receive_data[socket->receive_len - 1] == '\r')
                    {
                        socket->receive_len--;
                    }
                    complete = 1;
                    break;
                }
            }
            if (complete) {
                return hoplite_cosocket_complete_receive(socket, 1, NULL);
            }
            if (hoplite_cosocket_receive_append(socket,
                                                 socket->leftover,
                                                 socket->leftover_len) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            socket->leftover_len = 0;
        } else {
            if (hoplite_cosocket_receive_append(socket,
                                                 socket->leftover,
                                                 socket->leftover_len) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            socket->leftover_len = 0;
        }
    }

    for ( ;; ) {
        if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_FIXED) {
            amount = socket->receive_target - socket->receive_len;
            if (amount > sizeof(chunk)) {
                amount = sizeof(chunk);
            }
        } else if (socket->receive_mode
                   == HOPLITE_COSOCKET_RECEIVE_ANY)
        {
            amount = socket->receive_target;
            if (amount > sizeof(chunk)) {
                amount = sizeof(chunk);
            }
        } else {
            amount = sizeof(chunk);
            if (socket->receive_len == HOPLITE_COSOCKET_MAX_IO) {
                amount = 1;
            }
        }

        received = socket->connection->recv(socket->connection, chunk, amount);
        if (received > 0) {
            if (socket->receive_len == HOPLITE_COSOCKET_MAX_IO) {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_ANY) {
                if (hoplite_cosocket_receive_append(
                        socket, chunk, (size_t) received) != NGX_OK)
                {
                    hoplite_cosocket_reset_connection(socket);
                    return hoplite_cosocket_complete_receive(
                        socket, 0, "buffer too small");
                }
                return hoplite_cosocket_complete_receive(socket, 1, NULL);
            }
            if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_LINE) {
                if (hoplite_cosocket_receive_line_data(
                        socket, chunk, (size_t) received, &complete) != NGX_OK)
                {
                    hoplite_cosocket_reset_connection(socket);
                    return hoplite_cosocket_complete_receive(
                        socket, 0, "buffer too small");
                }
                if (complete) {
                    return hoplite_cosocket_complete_receive(socket, 1, NULL);
                }
                if (socket->receive_len == HOPLITE_COSOCKET_MAX_IO) {
                    hoplite_cosocket_reset_connection(socket);
                    return hoplite_cosocket_complete_receive(
                        socket, 0, "buffer too small");
                }
            } else if (hoplite_cosocket_receive_append(
                           socket, chunk, (size_t) received) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_FIXED
                && socket->receive_len == socket->receive_target)
            {
                return hoplite_cosocket_complete_receive(socket, 1, NULL);
            }
            continue;
        }
        if (received == NGX_AGAIN) {
            if (hoplite_cosocket_arm_read(socket) != NGX_OK) {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "receive failed");
            }
            return NGX_AGAIN;
        }
        if (received == 0) {
            ngx_flag_t all = socket->receive_mode
                == HOPLITE_COSOCKET_RECEIVE_ALL;
            hoplite_cosocket_reset_connection(socket);
            return hoplite_cosocket_complete_receive(
                socket, all, all ? NULL : "closed");
        }
        {
            const char *message = hoplite_cosocket_error_text(
                ngx_socket_errno, "receive failed");
            hoplite_cosocket_reset_connection(socket);
            return hoplite_cosocket_complete_receive(socket, 0, message);
        }
    }
}

static void
hoplite_cosocket_write_handler(ngx_event_t *event)
{
    ngx_connection_t *connection = event->data;
    hoplite_cosocket_t *socket = connection == NULL ? NULL : connection->data;

    if (socket == NULL || socket->released || socket->closed
        || socket->connection != connection)
    {
        return;
    }
    if (event->timedout) {
        hoplite_cosocket_reset_connection(socket);
        (void) hoplite_cosocket_complete_ordinary(socket, 0, 0, "timeout");
        return;
    }
    if (socket->pending == HOPLITE_COSOCKET_PENDING_CONNECT) {
        (void) hoplite_cosocket_connect_check(socket);
    } else if (socket->pending == HOPLITE_COSOCKET_PENDING_SEND) {
        (void) hoplite_cosocket_send_drive(socket);
    }
}

static void
hoplite_cosocket_read_handler(ngx_event_t *event)
{
    ngx_connection_t *connection = event->data;
    hoplite_cosocket_t *socket = connection == NULL ? NULL : connection->data;

    if (socket == NULL || socket->released || socket->closed
        || socket->connection != connection
        || socket->pending != HOPLITE_COSOCKET_PENDING_RECEIVE)
    {
        return;
    }
    if (event->timedout) {
        event->timedout = 0;
        (void) hoplite_cosocket_complete_receive(socket, 0, "timeout");
        return;
    }
    (void) hoplite_cosocket_receive_drive(socket);
}

static ngx_flag_t
hoplite_cosocket_operation(const hoplite_host_call_v1_t *call,
                           const char *name)
{
    size_t len = ngx_strlen(name);
    return call->operation.len == len
        && ngx_strncmp(call->operation.data, name, len) == 0;
}

static ngx_int_t
hoplite_cosocket_decode_arguments(const hoplite_host_call_v1_t *call,
                                  ngx_pool_t *pool,
                                  hoplite_hta_value_t **arguments)
{
    if (call == NULL || pool == NULL || arguments == NULL
        || call->arguments_hta.data == NULL
        || call->arguments_hta.len == 0
        || hoplite_hta_decode(pool,
                              call->arguments_hta.data,
                              call->arguments_hta.len,
                              arguments) != NGX_OK
        || *arguments == NULL
        || (*arguments)->kind != HOPLITE_HTA_VECTOR)
    {
        return NGX_ERROR;
    }
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_argument_handle(const hoplite_host_call_v1_t *call,
                                 const hoplite_hta_value_t *arguments,
                                 size_t count,
                                 hoplite_cosocket_t **socket)
{
    int64_t handle;

    if (arguments == NULL || arguments->as.vector.count != count
        || hoplite_hta_number(arguments->as.vector.items[0], &handle) != NGX_OK
        || handle <= 0)
    {
        return NGX_ERROR;
    }
    *socket = hoplite_cosocket_find(call->request_context,
                                    call->work,
                                    (uint64_t) handle);
    return *socket == NULL ? NGX_DECLINED : NGX_OK;
}

static ngx_int_t
hoplite_cosocket_tcp(const hoplite_host_call_v1_t *call,
                     const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;

    if (arguments->as.vector.count != 0) {
        return hoplite_cosocket_reject(call,
                                       "hoplite.socket/tcp expects no arguments");
    }
    if (hoplite_cosocket_next_id == UINT64_MAX) {
        return hoplite_cosocket_reject(call,
                                       "hoplite cosocket handle space exhausted");
    }
    socket = ngx_alloc(sizeof(*socket), hoplite_cosocket_log);
    if (socket == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    ngx_memzero(socket, sizeof(*socket));
    socket->pool = ngx_create_pool(4096, hoplite_cosocket_log);
    if (socket->pool == NULL) {
        ngx_free(socket);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    socket->id = ++hoplite_cosocket_next_id;
    socket->owner = call->request_context;
    socket->work = call->work;
    socket->log = hoplite_cosocket_log;
    socket->connect_timeout = HOPLITE_COSOCKET_DEFAULT_TIMEOUT;
    socket->send_timeout = HOPLITE_COSOCKET_DEFAULT_TIMEOUT;
    socket->read_timeout = HOPLITE_COSOCKET_DEFAULT_TIMEOUT;
    if (hoplite_host_request_cleanup_add_v1(
            call->request_context,
            call->work,
            socket,
            hoplite_cosocket_request_cleanup) != HOPLITE_HOST_RESOURCE_OK)
    {
        ngx_destroy_pool(socket->pool);
        ngx_free(socket);
        return hoplite_cosocket_reject(
            call, "could not register Hoplite cosocket cleanup");
    }
    socket->next = hoplite_cosockets;
    hoplite_cosockets = socket;
    if (hoplite_cosocket_complete_number(call, (int64_t) socket->id) != NGX_OK) {
        hoplite_cosocket_close_state(socket);
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    return HOPLITE_HOST_PROVIDER_OK;
}

static ngx_int_t
hoplite_cosocket_connect(const hoplite_host_call_v1_t *call,
                         const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    hoplite_hta_value_t *options;
    ngx_url_t url;
    ngx_str_t host, address;
    u_char *cursor;
    size_t capacity;
    int64_t port;
    ngx_int_t rc;
    ngx_flag_t ipv6 = 0;
    size_t index;

    rc = hoplite_cosocket_argument_handle(call, arguments, 4, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/connect expects [socket host port options]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
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
            call, "cosocket connect options are reserved for pooling and DNS");
    }
    for (index = 0; index < host.len; index++) {
        if (host.data[index] == ':') {
            ipv6 = 1;
            break;
        }
    }
    capacity = host.len + 32;
    address.data = ngx_pnalloc(socket->pool, capacity);
    if (address.data == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    cursor = ipv6
        ? ngx_snprintf(address.data, capacity, "[%V]:%L", &host, port)
        : ngx_snprintf(address.data, capacity, "%V:%L", &host, port);
    address.len = (size_t) (cursor - address.data);

    ngx_memzero(&url, sizeof(url));
    url.url = address;
    url.no_resolve = 1;
    if (ngx_parse_url(socket->pool, &url) != NGX_OK || url.naddrs != 1) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "numeric address required");
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

static ngx_int_t
hoplite_cosocket_send(const hoplite_host_call_v1_t *call,
                      const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    ngx_str_t value;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/send expects [socket string-or-bytes]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed || !socket->connected || socket->connection == NULL) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (hoplite_hta_text(arguments->as.vector.items[1], &value) != NGX_OK
        || value.len > HOPLITE_COSOCKET_MAX_IO)
    {
        return hoplite_cosocket_reject(
            call, "cosocket send value must be at most 1048576 bytes");
    }
    if (value.len != 0) {
        socket->send_data = ngx_alloc(value.len, socket->log);
        if (socket->send_data == NULL) {
            return HOPLITE_HOST_PROVIDER_ERROR;
        }
        ngx_memcpy(socket->send_data, value.data, value.len);
    }
    socket->send_len = value.len;
    socket->send_offset = 0;
    socket->pending = HOPLITE_COSOCKET_PENDING_SEND;
    socket->pending_call = call->call;
    socket->completer = call->completer;
    rc = hoplite_cosocket_send_drive(socket);
    if (rc == NGX_AGAIN) {
        return HOPLITE_HOST_PROVIDER_PENDING;
    }
    return rc == NGX_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static ngx_int_t
hoplite_cosocket_receive(const hoplite_host_call_v1_t *call,
                         const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    hoplite_hta_value_t *pattern;
    ngx_str_t text;
    int64_t count;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/receive expects [socket pattern]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed || !socket->connected || socket->connection == NULL) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    pattern = arguments->as.vector.items[1];
    if (hoplite_hta_number(pattern, &count) == NGX_OK) {
        if (count < 1 || count > HOPLITE_COSOCKET_MAX_IO) {
            return hoplite_cosocket_reject(
                call, "fixed receive size must be between 1 and 1048576");
        }
        socket->receive_mode = HOPLITE_COSOCKET_RECEIVE_FIXED;
        socket->receive_target = (size_t) count;
    } else if (hoplite_hta_text(pattern, &text) == NGX_OK
               && text.len == 2 && text.data[0] == '*'
               && (text.data[1] == 'l' || text.data[1] == 'a'))
    {
        socket->receive_mode = text.data[1] == 'l'
            ? HOPLITE_COSOCKET_RECEIVE_LINE
            : HOPLITE_COSOCKET_RECEIVE_ALL;
    } else {
        return hoplite_cosocket_reject(
            call, "receive pattern must be a positive count, *l, or *a");
    }
    socket->pending = HOPLITE_COSOCKET_PENDING_RECEIVE;
    socket->pending_call = call->call;
    socket->completer = call->completer;
    rc = hoplite_cosocket_receive_drive(socket);
    if (rc == NGX_AGAIN) {
        return HOPLITE_HOST_PROVIDER_PENDING;
    }
    return rc == NGX_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static ngx_int_t
hoplite_cosocket_receiveany(const hoplite_host_call_v1_t *call,
                            const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    int64_t maximum;
    ngx_int_t rc;

    rc = hoplite_cosocket_argument_handle(call, arguments, 2, &socket);
    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/receiveany expects [socket maximum]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed || !socket->connected || socket->connection == NULL) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (hoplite_hta_number(arguments->as.vector.items[1], &maximum) != NGX_OK
        || maximum < 1 || maximum > HOPLITE_COSOCKET_MAX_IO)
    {
        return hoplite_cosocket_reject(
            call, "receiveany maximum must be between 1 and 1048576");
    }

    socket->receive_mode = HOPLITE_COSOCKET_RECEIVE_ANY;
    socket->receive_target = (size_t) maximum;
    socket->pending = HOPLITE_COSOCKET_PENDING_RECEIVE;
    socket->pending_call = call->call;
    socket->completer = call->completer;
    rc = hoplite_cosocket_receive_drive(socket);
    if (rc == NGX_AGAIN) {
        return HOPLITE_HOST_PROVIDER_PENDING;
    }
    return rc == NGX_OK
        ? HOPLITE_HOST_PROVIDER_OK
        : HOPLITE_HOST_PROVIDER_ERROR;
}

static ngx_int_t
hoplite_cosocket_close(const hoplite_host_call_v1_t *call,
                       const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    ngx_int_t rc = hoplite_cosocket_argument_handle(call, arguments, 1, &socket);

    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/close expects [socket]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    hoplite_cosocket_close_state(socket);
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_timeout_value(hoplite_hta_value_t *value,
                               ngx_msec_t *output)
{
    int64_t number;
    if (hoplite_hta_number(value, &number) != NGX_OK
        || number < 0 || number > HOPLITE_COSOCKET_MAX_TIMEOUT)
    {
        return NGX_ERROR;
    }
    *output = (ngx_msec_t) number;
    return NGX_OK;
}

static ngx_int_t
hoplite_cosocket_settimeout(const hoplite_host_call_v1_t *call,
                            const hoplite_hta_value_t *arguments,
                            ngx_flag_t separate)
{
    hoplite_cosocket_t *socket;
    ngx_msec_t connect_timeout, send_timeout, read_timeout;
    size_t count = separate ? 4 : 2;
    ngx_int_t rc = hoplite_cosocket_argument_handle(
        call, arguments, count, &socket);

    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call,
            separate
                ? "hoplite.socket/settimeouts expects [socket connect send read]"
                : "hoplite.socket/settimeout expects [socket milliseconds]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    if (separate) {
        if (hoplite_cosocket_timeout_value(arguments->as.vector.items[1],
                                           &connect_timeout) != NGX_OK
            || hoplite_cosocket_timeout_value(arguments->as.vector.items[2],
                                               &send_timeout) != NGX_OK
            || hoplite_cosocket_timeout_value(arguments->as.vector.items[3],
                                               &read_timeout) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "cosocket timeouts must be between 0 and 3600000 ms");
        }
    } else {
        if (hoplite_cosocket_timeout_value(arguments->as.vector.items[1],
                                           &connect_timeout) != NGX_OK)
        {
            return hoplite_cosocket_reject(
                call, "cosocket timeout must be between 0 and 3600000 ms");
        }
        send_timeout = connect_timeout;
        read_timeout = connect_timeout;
    }
    socket->connect_timeout = connect_timeout;
    socket->send_timeout = send_timeout;
    socket->read_timeout = read_timeout;
    return hoplite_cosocket_complete_ordinary_call(call, 1, 1, NULL);
}

static ngx_int_t
hoplite_cosocket_getreusedtimes(const hoplite_host_call_v1_t *call,
                                const hoplite_hta_value_t *arguments)
{
    hoplite_cosocket_t *socket;
    ngx_int_t rc = hoplite_cosocket_argument_handle(call, arguments, 1, &socket);

    if (rc == NGX_ERROR) {
        return hoplite_cosocket_reject(
            call, "hoplite.socket/getreusedtimes expects [socket]");
    }
    if (rc == NGX_DECLINED) {
        return hoplite_cosocket_reject(call,
                                       "unknown or foreign Hoplite cosocket");
    }
    if (socket->closed) {
        return hoplite_cosocket_complete_ordinary_call(
            call, 0, 0, "closed");
    }
    return hoplite_cosocket_complete_ordinary_call(call, 1, 0, NULL);
}

static int32_t
hoplite_cosocket_invoke(const hoplite_host_call_v1_t *call)
{
    ngx_pool_t *pool;
    hoplite_hta_value_t *arguments = NULL;
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
    pool = ngx_create_pool(4096, hoplite_cosocket_log);
    if (pool == NULL) {
        return HOPLITE_HOST_PROVIDER_ERROR;
    }
    if (hoplite_cosocket_decode_arguments(call, pool, &arguments) != NGX_OK) {
        ngx_destroy_pool(pool);
        return hoplite_cosocket_reject(
            call, "hoplite.socket arguments must be one HTA vector");
    }

    if (hoplite_cosocket_operation(call, "tcp")) {
        rc = hoplite_cosocket_tcp(call, arguments);
    } else if (hoplite_cosocket_operation(call, "connect")) {
        rc = hoplite_cosocket_connect(call, arguments);
    } else if (hoplite_cosocket_operation(call, "send")) {
        rc = hoplite_cosocket_send(call, arguments);
    } else if (hoplite_cosocket_operation(call, "receive")) {
        rc = hoplite_cosocket_receive(call, arguments);
    } else if (hoplite_cosocket_operation(call, "receiveany")) {
        rc = hoplite_cosocket_receiveany(call, arguments);
    } else if (hoplite_cosocket_operation(call, "close")) {
        rc = hoplite_cosocket_close(call, arguments);
    } else if (hoplite_cosocket_operation(call, "settimeout")) {
        rc = hoplite_cosocket_settimeout(call, arguments, 0);
    } else if (hoplite_cosocket_operation(call, "settimeouts")) {
        rc = hoplite_cosocket_settimeout(call, arguments, 1);
    } else if (hoplite_cosocket_operation(call, "getreusedtimes")) {
        rc = hoplite_cosocket_getreusedtimes(call, arguments);
    } else {
        rc = hoplite_cosocket_reject(call,
                                     "unsupported hoplite.socket operation");
    }
    ngx_destroy_pool(pool);
    return (int32_t) rc;
}

static void
hoplite_cosocket_cancel(void *request_context)
{
    hoplite_cosocket_t *socket;

    for (socket = hoplite_cosockets;
         socket != NULL;
         socket = socket->next)
    {
        if (socket->owner == request_context && !socket->released) {
            hoplite_cosocket_close_state(socket);
        }
    }
}

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
    rc = hoplite_host_provider_register_v1(&hoplite_cosocket_provider);
    return rc == HOPLITE_HOST_PROVIDER_REGISTER_OK ? NGX_OK : NGX_ERROR;
}

void
hoplite_cosocket_worker_exit(void)
{
    hoplite_cosocket_t *socket;

    while (hoplite_cosockets != NULL) {
        socket = hoplite_cosockets;
        hoplite_cosockets = socket->next;
        socket->next = NULL;
        if (!socket->released) {
            socket->released = 1;
            hoplite_cosocket_close_state(socket);
            if (socket->pool != NULL) {
                ngx_destroy_pool(socket->pool);
            }
            ngx_free(socket);
        }
    }
    hoplite_cosocket_next_id = 0;
    hoplite_cosocket_log = NULL;
}
