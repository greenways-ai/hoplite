#include "hoplite_console_worker.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#include "console_transport_fixture.inc"

typedef struct {
    uint64_t next_work;
    uint64_t app;
    uint8_t input[2048];
    size_t input_len;
    int calls;
    int cancels;
    int closes;
    int cancel_rc;
    int close_rc;
} fake_runtime_t;

typedef struct {
    fake_runtime_t runtime;
    hoplite_console_worker_t worker;
    uint8_t request[2048];
    uint8_t application[2048];
    uint8_t response[2052];
} fixture_t;

static uint64_t fake_call(
    void *context,
    uint64_t app,
    const uint8_t *input,
    size_t input_len)
{
    fake_runtime_t *runtime = context;
    runtime->calls++;
    runtime->app = app;
    runtime->input_len = input_len;
    assert(input_len <= sizeof(runtime->input));
    memcpy(runtime->input, input, input_len);
    return runtime->next_work;
}

static int fake_cancel(void *context, uint64_t work)
{
    fake_runtime_t *runtime = context;
    assert(work == runtime->next_work);
    runtime->cancels++;
    return runtime->cancel_rc;
}

static int fake_close(void *context, uint64_t work)
{
    fake_runtime_t *runtime = context;
    assert(work == runtime->next_work);
    runtime->closes++;
    return runtime->close_rc;
}

static void fixture_init(
    fixture_t *fixture,
    size_t request_bytes,
    size_t result_bytes)
{
    hoplite_console_worker_ops_t ops;
    hoplite_console_worker_limits_t limits;

    memset(fixture, 0, sizeof(*fixture));
    fixture->runtime.next_work = 41;
    ops.call = fake_call;
    ops.cancel = fake_cancel;
    ops.close = fake_close;
    limits.request_bytes = request_bytes;
    limits.result_bytes = result_bytes;
    assert(hoplite_console_worker_init(
        &fixture->worker,
        &fixture->runtime,
        7,
        ops,
        limits,
        fixture->request,
        sizeof(fixture->request),
        fixture->application,
        sizeof(fixture->application),
        fixture->response,
        sizeof(fixture->response)) == HOPLITE_CONSOLE_WORKER_OK);
}

static size_t frame_payload(
    const hoplite_console_slice_t *frame,
    const uint8_t **payload)
{
    size_t len;
    assert(frame != NULL);
    assert(frame->data != NULL);
    assert(frame->len >= 4);
    len = ((size_t) frame->data[0] << 24)
        | ((size_t) frame->data[1] << 16)
        | ((size_t) frame->data[2] << 8)
        | (size_t) frame->data[3];
    assert(len == frame->len - 4);
    *payload = frame->data + 4;
    return len;
}

static void assert_output_contains(
    const hoplite_console_worker_t *worker,
    const char *needle)
{
    hoplite_console_slice_t output;
    const uint8_t *payload;
    size_t payload_len;

    assert(hoplite_console_worker_output(worker, &output)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    payload_len = frame_payload(&output, &payload);
    assert(memcmp(payload, "HTA0", 4) == 0);
    assert(contains_bytes(
        payload,
        payload_len,
        (const uint8_t *) needle,
        strlen(needle)));
}

static int feed_call(
    fixture_t *fixture,
    const uint8_t *suffix,
    size_t suffix_len)
{
    uint8_t payload[1024];
    uint8_t frame[1032];
    size_t payload_len = write_call(payload, NULL, 0);
    size_t frame_len = 0;
    size_t consumed = 0;

    assert(hoplite_console_frame_encode(
        payload,
        payload_len,
        1024,
        frame,
        sizeof(frame),
        &frame_len) == HOPLITE_CONSOLE_TRANSPORT_OK);
    assert(frame_len + suffix_len <= sizeof(frame));
    if (suffix_len != 0) {
        memcpy(frame + frame_len, suffix, suffix_len);
    }
    return hoplite_console_worker_feed(
        &fixture->worker,
        frame,
        frame_len + suffix_len,
        &consumed);
}

static void test_fragmented_call_starts_only_the_closed_envelope(void)
{
    fixture_t fixture;
    uint8_t payload[1024];
    uint8_t frame[1028];
    size_t payload_len = write_call(payload, NULL, 0);
    size_t frame_len = 0;
    size_t consumed = 0;
    const uint8_t op_key[] = {HTA_KEYWORD, 0, 0, 0, 2, 'o', 'p'};

    fixture_init(&fixture, 1024, 1024);
    assert(hoplite_console_frame_encode(
        payload,
        payload_len,
        1024,
        frame,
        sizeof(frame),
        &frame_len) == 0);
    assert(hoplite_console_worker_feed(
        &fixture.worker, frame, 3, &consumed)
        == HOPLITE_CONSOLE_WORKER_MORE);
    assert(consumed == 3);
    assert(hoplite_console_worker_feed(
        &fixture.worker, frame + 3, 17, &consumed)
        == HOPLITE_CONSOLE_WORKER_MORE);
    assert(consumed == 17);
    assert(hoplite_console_worker_feed(
        &fixture.worker, frame + 20, frame_len - 20, &consumed)
        == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(consumed == frame_len - 20);
    assert(fixture.runtime.calls == 1);
    assert(fixture.runtime.app == 7);
    assert(hoplite_console_worker_work(&fixture.worker) == 41);
    assert(hoplite_console_worker_phase(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_RUNNING);
    assert(memcmp(fixture.runtime.input, "HTA0", 4) == 0);
    assert(hoplite_console_value_validate(
        fixture.runtime.input + 4,
        fixture.runtime.input_len - 4) == 0);
    assert(!contains_bytes(
        fixture.runtime.input,
        fixture.runtime.input_len,
        op_key,
        sizeof(op_key)));
    assert(!contains_bytes(
        fixture.runtime.input,
        fixture.runtime.input_len,
        (const uint8_t *) "handler",
        strlen("handler")));
}

static void test_extra_connection_bytes_fail_before_application_entry(void)
{
    fixture_t fixture;
    const uint8_t suffix = 0;

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, &suffix, 1) == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.calls == 0);
    assert_output_contains(&fixture.worker, "hoplite.console/request-invalid");
}

static void test_application_start_failure_is_structured(void)
{
    fixture_t fixture;

    fixture_init(&fixture, 1024, 1024);
    fixture.runtime.next_work = 0;
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.calls == 1);
    assert_output_contains(
        &fixture.worker,
        "hoplite.console/application-unavailable");
}

static void test_success_closes_once_and_supports_partial_writes(void)
{
    fixture_t fixture;
    uint8_t value[64];
    size_t value_len = write_text(value, HTA_STRING, "ready");
    hoplite_console_slice_t output;
    size_t first;

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(hoplite_console_worker_complete(
        &fixture.worker, value, value_len) == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.cancels == 0);
    assert(fixture.runtime.closes == 1);
    assert_output_contains(&fixture.worker, "ready");
    assert(hoplite_console_worker_output(&fixture.worker, &output)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    first = output.len / 2;
    assert(first != 0);
    assert(hoplite_console_worker_consume_output(&fixture.worker, first)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(hoplite_console_worker_output(&fixture.worker, &output)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(hoplite_console_worker_consume_output(&fixture.worker, output.len)
        == HOPLITE_CONSOLE_WORKER_DONE);
    assert(hoplite_console_worker_phase(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_FINISHED);
    assert(fixture.runtime.closes == 1);
}

static void test_collection_results_are_accepted(void)
{
    fixture_t fixture;
    uint8_t value[16];
    size_t value_len;

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    value_len = write_empty_map(value);
    assert(hoplite_console_worker_complete(
        &fixture.worker, value, value_len) == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.closes == 1);

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    value_len = write_sequence(value, HTA_VECTOR, 0);
    assert(hoplite_console_worker_complete(
        &fixture.worker, value, value_len) == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.closes == 1);
}

static void test_invalid_and_oversized_results_fail_closed(void)
{
    fixture_t fixture;
    uint8_t invalid[] = {HTA_ARRAY};
    uint8_t large[1500];
    size_t large_len;

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(hoplite_console_worker_complete(
        &fixture.worker, invalid, sizeof(invalid))
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert_output_contains(
        &fixture.worker,
        "hoplite.console/result-not-immutable");
    assert(fixture.runtime.closes == 1);

    fixture_init(&fixture, 1024, 256);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    memset(large + 5, 'x', 1000);
    large_len = write_text_bytes(
        large,
        HTA_STRING,
        large + 5,
        1000);
    assert(large_len == 1005);
    assert(hoplite_console_worker_complete(
        &fixture.worker, large, large_len)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert_output_contains(
        &fixture.worker,
        "hoplite.console/result-too-large");
    assert(fixture.runtime.closes == 1);
}

static void test_timeout_and_host_calls_cancel_before_writeback(void)
{
    fixture_t fixture;

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(hoplite_console_worker_timeout(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.cancels == 1);
    assert(fixture.runtime.closes == 1);
    assert_output_contains(&fixture.worker, "hoplite.console/timeout");

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(hoplite_console_worker_forbid_host_call(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.cancels == 1);
    assert(fixture.runtime.closes == 1);
    assert_output_contains(
        &fixture.worker,
        "hoplite.console/host-call-forbidden");
}

static void test_disconnect_cleanup_and_cancel_failure(void)
{
    fixture_t fixture;

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(hoplite_console_worker_abort(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_DONE);
    assert(fixture.runtime.cancels == 1);
    assert(fixture.runtime.closes == 1);
    assert(hoplite_console_worker_phase(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_FINISHED);

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    fixture.runtime.cancel_rc = -1;
    assert(hoplite_console_worker_abort(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_FATAL);
    assert(fixture.runtime.cancels == 1);
    assert(fixture.runtime.closes == 1);
    assert(hoplite_console_worker_phase(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_BROKEN);
}

static void test_failure_output_is_redacted_and_bounded(void)
{
    fixture_t fixture;
    uint8_t message[1500];

    memset(message, 'x', sizeof(message));
    fixture_init(&fixture, 1024, 256);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    assert(hoplite_console_worker_fail(
        &fixture.worker,
        (const uint8_t *) "provider/private",
        strlen("provider/private"),
        message,
        sizeof(message)) == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert_output_contains(&fixture.worker, "hoplite.console/failure");
    assert(!contains_bytes(
        fixture.response,
        fixture.worker.response_length,
        (const uint8_t *) "provider/private",
        strlen("provider/private")));
    assert(fixture.runtime.closes == 1);
}

static void test_close_failure_is_fatal(void)
{
    fixture_t fixture;
    uint8_t value[] = {HTA_NIL};

    fixture_init(&fixture, 1024, 1024);
    assert(feed_call(&fixture, NULL, 0) == HOPLITE_CONSOLE_WORKER_STARTED);
    fixture.runtime.close_rc = -1;
    assert(hoplite_console_worker_complete(
        &fixture.worker, value, sizeof(value))
        == HOPLITE_CONSOLE_WORKER_FATAL);
    assert(fixture.runtime.closes == 1);
    assert(hoplite_console_worker_phase(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_BROKEN);
}

static void test_initialization_rejects_incoherent_storage(void)
{
    fixture_t fixture;
    hoplite_console_worker_ops_t ops = {fake_call, fake_cancel, fake_close};
    hoplite_console_worker_limits_t limits = {1024, 1024};

    memset(&fixture, 0, sizeof(fixture));
    fixture.runtime.next_work = 41;
    assert(hoplite_console_worker_init(
        &fixture.worker,
        &fixture.runtime,
        7,
        ops,
        limits,
        fixture.request,
        100,
        fixture.application,
        sizeof(fixture.application),
        fixture.response,
        sizeof(fixture.response)) == HOPLITE_CONSOLE_WORKER_FATAL);
    assert(hoplite_console_worker_init(
        &fixture.worker,
        &fixture.runtime,
        7,
        ops,
        limits,
        fixture.request,
        sizeof(fixture.request),
        fixture.application,
        sizeof(fixture.application),
        fixture.response,
        100) == HOPLITE_CONSOLE_WORKER_FATAL);
}

static void test_read_timeout_does_not_touch_runtime_work(void)
{
    fixture_t fixture;

    fixture_init(&fixture, 1024, 1024);
    assert(hoplite_console_worker_timeout(&fixture.worker)
        == HOPLITE_CONSOLE_WORKER_OUTPUT);
    assert(fixture.runtime.calls == 0);
    assert(fixture.runtime.cancels == 0);
    assert(fixture.runtime.closes == 0);
    assert_output_contains(&fixture.worker, "hoplite.console/timeout");
}

int main(void)
{
    test_fragmented_call_starts_only_the_closed_envelope();
    test_extra_connection_bytes_fail_before_application_entry();
    test_application_start_failure_is_structured();
    test_success_closes_once_and_supports_partial_writes();
    test_collection_results_are_accepted();
    test_invalid_and_oversized_results_fail_closed();
    test_timeout_and_host_calls_cancel_before_writeback();
    test_disconnect_cleanup_and_cancel_failure();
    test_failure_output_is_redacted_and_bounded();
    test_close_failure_is_fatal();
    test_initialization_rejects_incoherent_storage();
    test_read_timeout_does_not_touch_runtime_work();
    puts("console worker tests passed");
    return 0;
}
