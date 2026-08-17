#include "hoplite_console_transport.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#include "console_transport_fixture.inc"

#include "console_transport_validation.inc"

static void test_result_envelopes_are_bounded_frames(void)
{
    uint8_t value[64];
    uint8_t payload[512];
    uint8_t frame[516];
    size_t value_len = write_text(value, HTA_STRING, "ready");
    size_t payload_len;
    size_t frame_len;
    const char *code = "hoplite.console/handler-failed";
    const char *message = "handler failed";

    assert(hoplite_console_success_encode(
        value, value_len, payload, sizeof(payload), &payload_len) == 0);
    assert(memcmp(payload, "HTA0", 4) == 0);
    assert(contains_bytes(
        payload, payload_len, (const uint8_t *) "ok", strlen("ok")));
    assert(contains_bytes(
        payload, payload_len, (const uint8_t *) "value", strlen("value")));
    assert(hoplite_console_frame_encode(
        payload, payload_len, payload_len, frame, sizeof(frame), &frame_len) == 0);
    assert(frame_len == payload_len + 4);
    assert(hoplite_console_frame_encode(
        payload, payload_len, payload_len - 1, frame, sizeof(frame), &frame_len) == -1);

    value[0] = HTA_ARRAY;
    assert(hoplite_console_success_encode(
        value, value_len, payload, sizeof(payload), &payload_len) == -1);

    assert(hoplite_console_failure_encode(
        (const uint8_t *) code, strlen(code),
        (const uint8_t *) message, strlen(message),
        payload, sizeof(payload), &payload_len) == 0);
    assert(memcmp(payload, "HTA0", 4) == 0);
    assert(contains_bytes(
        payload, payload_len, (const uint8_t *) code, strlen(code)));
    assert(contains_bytes(
        payload, payload_len, (const uint8_t *) message, strlen(message)));
}

static void test_random_inputs_fail_safely(void)
{
    uint8_t bytes[128];
    uint32_t state = 0x6d2b79f5u;
    size_t round;
    size_t index;
    hoplite_console_call_t call;

    for (round = 0; round < 10000; round++) {
        size_t len = round % sizeof(bytes);
        for (index = 0; index < len; index++) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            bytes[index] = (uint8_t) state;
        }
        (void) hoplite_console_value_validate(bytes, len);
        (void) hoplite_console_call_parse(bytes, len, &call);
    }
}

int main(void)
{
    test_fragmented_frame_reader();
    test_frame_bounds();
    test_exact_call_is_closed_before_application_entry();
    test_routing_and_duplicate_fields_fail_closed();
    test_full_immutable_hta0_grammar();
    test_live_and_mutable_hta0_tags_are_rejected();
    test_malformed_and_overdeep_values_fail_closed();
    test_result_envelopes_are_bounded_frames();
    test_random_inputs_fail_safely();
    puts("console transport tests passed");
    return 0;
}
