#include "hoplite_console_worker.h"

#include <string.h>

static const uint8_t request_invalid_code[] =
    "hoplite.console/request-invalid";
static const uint8_t request_invalid_message[] =
    "application console request is invalid";
static const uint8_t application_unavailable_code[] =
    "hoplite.console/application-unavailable";
static const uint8_t application_unavailable_message[] =
    "application console handler is unavailable";
static const uint8_t result_invalid_code[] =
    "hoplite.console/result-not-immutable";
static const uint8_t result_invalid_message[] =
    "application console result is not immutable HTA";
static const uint8_t result_large_code[] =
    "hoplite.console/result-too-large";
static const uint8_t result_large_message[] =
    "application console result exceeds its limit";
static const uint8_t timeout_code[] =
    "hoplite.console/timeout";
static const uint8_t timeout_message[] =
    "application console call timed out";
static const uint8_t host_call_code[] =
    "hoplite.console/host-call-forbidden";
static const uint8_t host_call_message[] =
    "application console commands cannot invoke host services";
static const uint8_t fallback_code[] =
    "hoplite.console/failure";
static const uint8_t fallback_message[] =
    "application console call failed";

static int worker_break(hoplite_console_worker_t *worker)
{
    if (worker != NULL) {
        worker->phase = HOPLITE_CONSOLE_WORKER_BROKEN;
    }
    return HOPLITE_CONSOLE_WORKER_FATAL;
}

static int worker_close(hoplite_console_worker_t *worker)
{
    if (worker == NULL) {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker->work == 0 || worker->work_closed) {
        return HOPLITE_CONSOLE_WORKER_OK;
    }
    worker->work_closed = 1;
    if (worker->ops.close(worker->runtime_context, worker->work) != 0) {
        return worker_break(worker);
    }
    return HOPLITE_CONSOLE_WORKER_OK;
}

static int worker_cancel_and_close(hoplite_console_worker_t *worker)
{
    int cancel_rc = 0;
    int close_rc = 0;

    if (worker == NULL) {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker->work == 0 || worker->work_closed) {
        return HOPLITE_CONSOLE_WORKER_OK;
    }
    if (!worker->cancel_attempted) {
        worker->cancel_attempted = 1;
        cancel_rc = worker->ops.cancel(worker->runtime_context, worker->work);
    }
    worker->work_closed = 1;
    close_rc = worker->ops.close(worker->runtime_context, worker->work);
    if (cancel_rc != 0 || close_rc != 0) {
        return worker_break(worker);
    }
    return HOPLITE_CONSOLE_WORKER_OK;
}

static int worker_store_payload(
    hoplite_console_worker_t *worker,
    size_t payload_length)
{
    uint8_t *prefix;
    if (worker == NULL || payload_length == 0
        || payload_length > worker->limits.result_bytes
        || payload_length > UINT32_MAX
        || worker->response_capacity < HOPLITE_CONSOLE_FRAME_PREFIX_BYTES
        || payload_length > worker->response_capacity
               - HOPLITE_CONSOLE_FRAME_PREFIX_BYTES)
    {
        return worker_break(worker);
    }
    prefix = worker->response_storage;
    prefix[0] = (uint8_t) (payload_length >> 24);
    prefix[1] = (uint8_t) (payload_length >> 16);
    prefix[2] = (uint8_t) (payload_length >> 8);
    prefix[3] = (uint8_t) payload_length;
    worker->response_length = payload_length
        + HOPLITE_CONSOLE_FRAME_PREFIX_BYTES;
    worker->response_offset = 0;
    worker->phase = HOPLITE_CONSOLE_WORKER_WRITING;
    return HOPLITE_CONSOLE_WORKER_OUTPUT;
}

/* Encode the HTA payload after a four-byte reserved frame prefix. */
static int worker_encode_failure(
    hoplite_console_worker_t *worker,
    const uint8_t *code,
    size_t code_len,
    const uint8_t *message,
    size_t message_len)
{
    size_t payload = 0;
    uint8_t *destination;
    size_t capacity;

    if (worker == NULL
        || worker->response_capacity <= HOPLITE_CONSOLE_FRAME_PREFIX_BYTES)
    {
        return worker_break(worker);
    }
    if (code == NULL || code_len == 0
        || (message_len != 0 && message == NULL))
    {
        code = fallback_code;
        code_len = sizeof(fallback_code) - 1;
        message = fallback_message;
        message_len = sizeof(fallback_message) - 1;
    }
    destination = worker->response_storage + HOPLITE_CONSOLE_FRAME_PREFIX_BYTES;
    capacity = worker->response_capacity - HOPLITE_CONSOLE_FRAME_PREFIX_BYTES;
    if (hoplite_console_failure_encode(
            code,
            code_len,
            message,
            message_len,
            destination,
            capacity,
            &payload) != HOPLITE_CONSOLE_TRANSPORT_OK
        || payload > worker->limits.result_bytes)
    {
        if (hoplite_console_failure_encode(
                fallback_code,
                sizeof(fallback_code) - 1,
                fallback_message,
                sizeof(fallback_message) - 1,
                destination,
                capacity,
                &payload) != HOPLITE_CONSOLE_TRANSPORT_OK
            || payload > worker->limits.result_bytes)
        {
            return worker_break(worker);
        }
    }
    return worker_store_payload(worker, payload);
}

static int worker_request_failure(hoplite_console_worker_t *worker)
{
    return worker_encode_failure(
        worker,
        request_invalid_code,
        sizeof(request_invalid_code) - 1,
        request_invalid_message,
        sizeof(request_invalid_message) - 1);
}

int hoplite_console_worker_init(
    hoplite_console_worker_t *worker,
    void *runtime_context,
    uint64_t app,
    hoplite_console_worker_ops_t ops,
    hoplite_console_worker_limits_t limits,
    uint8_t *request_storage,
    size_t request_capacity,
    uint8_t *application_storage,
    size_t application_capacity,
    uint8_t *response_storage,
    size_t response_capacity)
{
    if (worker == NULL || runtime_context == NULL || app == 0
        || ops.call == NULL || ops.cancel == NULL || ops.close == NULL
        || limits.request_bytes == 0 || limits.result_bytes == 0
        || limits.request_bytes > UINT32_MAX || limits.result_bytes > UINT32_MAX
        || request_storage == NULL || request_capacity < limits.request_bytes
        || application_storage == NULL
        || application_capacity < limits.request_bytes
        || response_storage == NULL
        || response_capacity < HOPLITE_CONSOLE_FRAME_PREFIX_BYTES
        || response_capacity - HOPLITE_CONSOLE_FRAME_PREFIX_BYTES
               < limits.result_bytes)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }

    memset(worker, 0, sizeof(*worker));
    worker->runtime_context = runtime_context;
    worker->app = app;
    worker->ops = ops;
    worker->limits = limits;
    worker->request_storage = request_storage;
    worker->request_capacity = request_capacity;
    worker->application_storage = application_storage;
    worker->application_capacity = application_capacity;
    worker->response_storage = response_storage;
    worker->response_capacity = response_capacity;
    worker->phase = HOPLITE_CONSOLE_WORKER_READING;
    hoplite_console_frame_reader_init(
        &worker->reader,
        request_storage,
        limits.request_bytes);
    if (worker->reader.failed) {
        return worker_break(worker);
    }
    return HOPLITE_CONSOLE_WORKER_OK;
}

int hoplite_console_worker_feed(
    hoplite_console_worker_t *worker,
    const uint8_t *input,
    size_t input_len,
    size_t *consumed)
{
    hoplite_console_slice_t payload;
    hoplite_console_call_t call;
    int rc;

    if (consumed != NULL) {
        *consumed = 0;
    }
    if (worker == NULL || consumed == NULL
        || worker->phase != HOPLITE_CONSOLE_WORKER_READING)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    rc = hoplite_console_frame_reader_feed(
        &worker->reader, input, input_len, consumed);
    if (rc == HOPLITE_CONSOLE_TRANSPORT_MORE) {
        return HOPLITE_CONSOLE_WORKER_MORE;
    }
    if (rc != HOPLITE_CONSOLE_TRANSPORT_OK || *consumed != input_len) {
        return worker_request_failure(worker);
    }

    payload = hoplite_console_frame_reader_payload(&worker->reader);
    if (hoplite_console_call_parse(payload.data, payload.len, &call)
            != HOPLITE_CONSOLE_TRANSPORT_OK
        || hoplite_console_call_encode(
               &call,
               worker->application_storage,
               worker->application_capacity,
               &worker->application_length)
               != HOPLITE_CONSOLE_TRANSPORT_OK
        || worker->application_length > worker->limits.request_bytes)
    {
        return worker_request_failure(worker);
    }

    worker->work = worker->ops.call(
        worker->runtime_context,
        worker->app,
        worker->application_storage,
        worker->application_length);
    if (worker->work == 0) {
        return worker_encode_failure(
            worker,
            application_unavailable_code,
            sizeof(application_unavailable_code) - 1,
            application_unavailable_message,
            sizeof(application_unavailable_message) - 1);
    }
    worker->phase = HOPLITE_CONSOLE_WORKER_RUNNING;
    return HOPLITE_CONSOLE_WORKER_STARTED;
}

int hoplite_console_worker_complete(
    hoplite_console_worker_t *worker,
    const uint8_t *value,
    size_t value_len)
{
    size_t payload = 0;
    uint8_t *destination;
    size_t capacity;

    if (worker == NULL
        || worker->phase != HOPLITE_CONSOLE_WORKER_RUNNING)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker_close(worker) != HOPLITE_CONSOLE_WORKER_OK) {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (hoplite_console_value_validate(value, value_len)
        != HOPLITE_CONSOLE_TRANSPORT_OK)
    {
        return worker_encode_failure(
            worker,
            result_invalid_code,
            sizeof(result_invalid_code) - 1,
            result_invalid_message,
            sizeof(result_invalid_message) - 1);
    }

    destination = worker->response_storage + HOPLITE_CONSOLE_FRAME_PREFIX_BYTES;
    capacity = worker->response_capacity - HOPLITE_CONSOLE_FRAME_PREFIX_BYTES;
    if (hoplite_console_success_encode(
            value,
            value_len,
            destination,
            capacity,
            &payload) != HOPLITE_CONSOLE_TRANSPORT_OK
        || payload > worker->limits.result_bytes)
    {
        return worker_encode_failure(
            worker,
            result_large_code,
            sizeof(result_large_code) - 1,
            result_large_message,
            sizeof(result_large_message) - 1);
    }
    return worker_store_payload(worker, payload);
}

int hoplite_console_worker_fail(
    hoplite_console_worker_t *worker,
    const uint8_t *code,
    size_t code_len,
    const uint8_t *message,
    size_t message_len)
{
    if (worker == NULL
        || worker->phase != HOPLITE_CONSOLE_WORKER_RUNNING)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker_close(worker) != HOPLITE_CONSOLE_WORKER_OK) {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    return worker_encode_failure(
        worker, code, code_len, message, message_len);
}

int hoplite_console_worker_forbid_host_call(
    hoplite_console_worker_t *worker)
{
    if (worker == NULL
        || worker->phase != HOPLITE_CONSOLE_WORKER_RUNNING)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker_cancel_and_close(worker) != HOPLITE_CONSOLE_WORKER_OK) {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    return worker_encode_failure(
        worker,
        host_call_code,
        sizeof(host_call_code) - 1,
        host_call_message,
        sizeof(host_call_message) - 1);
}

int hoplite_console_worker_timeout(
    hoplite_console_worker_t *worker)
{
    if (worker == NULL
        || (worker->phase != HOPLITE_CONSOLE_WORKER_READING
            && worker->phase != HOPLITE_CONSOLE_WORKER_RUNNING))
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker->phase == HOPLITE_CONSOLE_WORKER_RUNNING
        && worker_cancel_and_close(worker) != HOPLITE_CONSOLE_WORKER_OK)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    return worker_encode_failure(
        worker,
        timeout_code,
        sizeof(timeout_code) - 1,
        timeout_message,
        sizeof(timeout_message) - 1);
}

int hoplite_console_worker_abort(hoplite_console_worker_t *worker)
{
    if (worker == NULL || worker->phase == HOPLITE_CONSOLE_WORKER_BROKEN) {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    if (worker->phase == HOPLITE_CONSOLE_WORKER_RUNNING
        && worker_cancel_and_close(worker) != HOPLITE_CONSOLE_WORKER_OK)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    worker->phase = HOPLITE_CONSOLE_WORKER_FINISHED;
    worker->response_length = 0;
    worker->response_offset = 0;
    return HOPLITE_CONSOLE_WORKER_DONE;
}

int hoplite_console_worker_output(
    const hoplite_console_worker_t *worker,
    hoplite_console_slice_t *output)
{
    if (output != NULL) {
        output->data = NULL;
        output->len = 0;
    }
    if (worker == NULL || output == NULL
        || worker->phase != HOPLITE_CONSOLE_WORKER_WRITING
        || worker->response_offset > worker->response_length)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    output->data = worker->response_storage + worker->response_offset;
    output->len = worker->response_length - worker->response_offset;
    return output->len == 0
        ? HOPLITE_CONSOLE_WORKER_DONE
        : HOPLITE_CONSOLE_WORKER_OUTPUT;
}

int hoplite_console_worker_consume_output(
    hoplite_console_worker_t *worker,
    size_t amount)
{
    size_t remaining;
    if (worker == NULL
        || worker->phase != HOPLITE_CONSOLE_WORKER_WRITING
        || worker->response_offset > worker->response_length)
    {
        return HOPLITE_CONSOLE_WORKER_FATAL;
    }
    remaining = worker->response_length - worker->response_offset;
    if (amount == 0 || amount > remaining) {
        return worker_break(worker);
    }
    worker->response_offset += amount;
    if (worker->response_offset == worker->response_length) {
        worker->phase = HOPLITE_CONSOLE_WORKER_FINISHED;
        return HOPLITE_CONSOLE_WORKER_DONE;
    }
    return HOPLITE_CONSOLE_WORKER_OUTPUT;
}

uint64_t hoplite_console_worker_work(
    const hoplite_console_worker_t *worker)
{
    return worker == NULL ? 0 : worker->work;
}

hoplite_console_worker_phase_t hoplite_console_worker_phase(
    const hoplite_console_worker_t *worker)
{
    return worker == NULL
        ? HOPLITE_CONSOLE_WORKER_BROKEN
        : worker->phase;
}
