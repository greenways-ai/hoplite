#include "../hoplite_response_source.h"

#include <stdio.h>
#include <string.h>

#define CHECK(condition)                                                      \
    do {                                                                      \
        if (!(condition)) {                                                   \
            fprintf(stderr, "check failed at %s:%d: %s\n",                   \
                    __FILE__, __LINE__, #condition);                          \
            return 1;                                                         \
        }                                                                     \
    } while (0)

typedef struct {
    const uint8_t *bytes;
    size_t length;
    size_t cursor;
    size_t reads;
    size_t closes;
    size_t maximum_capacity;
    uint64_t expected_work;
    uint64_t expected_handle;
    int fail_read;
    int overread;
    int fail_close;
} fixture_t;

static int32_t
fixture_read(void *request_context,
             uint64_t work,
             uint64_t source_handle,
             uint8_t *output,
             size_t capacity,
             size_t *returned)
{
    fixture_t *fixture = request_context;
    size_t available, amount;

    if (returned != NULL) {
        *returned = 0;
    }
    if (fixture == NULL || output == NULL || returned == NULL
        || work != fixture->expected_work
        || source_handle != fixture->expected_handle
        || fixture->fail_read)
    {
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }

    fixture->reads++;
    if (capacity > fixture->maximum_capacity) {
        fixture->maximum_capacity = capacity;
    }
    if (fixture->overread) {
        *returned = capacity + 1;
        return HOPLITE_RESPONSE_SOURCE_OK;
    }

    available = fixture->length - fixture->cursor;
    amount = capacity < available ? capacity : available;
    if (amount != 0) {
        memcpy(output, fixture->bytes + fixture->cursor, amount);
        fixture->cursor += amount;
    }
    *returned = amount;
    return HOPLITE_RESPONSE_SOURCE_OK;
}

static int32_t
fixture_close(void *request_context,
              uint64_t work,
              uint64_t source_handle)
{
    fixture_t *fixture = request_context;

    if (fixture == NULL
        || work != fixture->expected_work
        || source_handle != fixture->expected_handle)
    {
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }
    fixture->closes++;
    return fixture->fail_close
        ? HOPLITE_RESPONSE_SOURCE_ERROR
        : HOPLITE_RESPONSE_SOURCE_OK;
}

static hoplite_response_source_descriptor_v1_t
descriptor(uint64_t handle, uint64_t offset, uint64_t length)
{
    static const uint8_t protocol[] = HOPLITE_RESPONSE_SOURCE_PROTOCOL;
    hoplite_response_source_descriptor_v1_t value;

    value.protocol = protocol;
    value.protocol_len = sizeof(protocol) - 1;
    value.source_handle = handle;
    value.offset = offset;
    value.length = length;
    return value;
}

static fixture_t
fixture(const uint8_t *bytes, size_t length, uint64_t work, uint64_t handle)
{
    fixture_t value;

    memset(&value, 0, sizeof(value));
    value.bytes = bytes;
    value.length = length;
    value.expected_work = work;
    value.expected_handle = handle;
    return value;
}

static int
test_descriptor_validation(void)
{
    hoplite_response_source_descriptor_v1_t value = descriptor(17, 4, 9);
    static const uint8_t wrong_protocol[] = "hara.response-source/2";

    CHECK(hoplite_response_source_descriptor_validate_v1(&value)
          == HOPLITE_RESPONSE_SOURCE_OK);

    value.protocol = wrong_protocol;
    value.protocol_len = sizeof(wrong_protocol) - 1;
    CHECK(hoplite_response_source_descriptor_validate_v1(&value)
          == HOPLITE_RESPONSE_SOURCE_ERROR);

    value = descriptor(0, 0, 1);
    CHECK(hoplite_response_source_descriptor_validate_v1(&value)
          == HOPLITE_RESPONSE_SOURCE_ERROR);

    value = descriptor(1, 0, 0);
    CHECK(hoplite_response_source_descriptor_validate_v1(&value)
          == HOPLITE_RESPONSE_SOURCE_ERROR);

    value = descriptor(1,
                       HOPLITE_RESPONSE_SOURCE_SAFE_INTEGER_MAX,
                       1);
    CHECK(hoplite_response_source_descriptor_validate_v1(&value)
          == HOPLITE_RESPONSE_SOURCE_ERROR);

    value = descriptor(HOPLITE_RESPONSE_SOURCE_SAFE_INTEGER_MAX + UINT64_C(1),
                       0,
                       1);
    CHECK(hoplite_response_source_descriptor_validate_v1(&value)
          == HOPLITE_RESPONSE_SOURCE_ERROR);
    return 0;
}

static int
test_bounded_resume_and_final_close(void)
{
    static const uint8_t bytes[] = "123456789";
    hoplite_response_source_descriptor_v1_t value = descriptor(17, 4, 9);
    hoplite_response_source_state_v1_t state;
    fixture_t source = fixture(bytes, sizeof(bytes) - 1, 29, 17);
    uint8_t output[4];
    size_t returned;
    uint8_t last;

    CHECK(hoplite_response_source_init_v1(
              &state, &source, 29, &value, fixture_read, fixture_close)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(state.cursor == 4);
    CHECK(state.remaining == 9);

    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(returned == 4 && last == 0);
    CHECK(memcmp(output, "1234", 4) == 0);
    CHECK(state.cursor == 8 && state.remaining == 5);

    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(returned == 4 && last == 0);
    CHECK(memcmp(output, "5678", 4) == 0);

    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(returned == 1 && last == 1 && output[0] == '9');
    CHECK(state.cursor == 13 && state.remaining == 0);
    CHECK(source.reads == 3);
    CHECK(source.maximum_capacity == 4);
    CHECK(source.closes == 1);

    CHECK(hoplite_response_source_close_v1(&state)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(source.closes == 1);
    return 0;
}

static int
test_head_closes_without_reading(void)
{
    static const uint8_t bytes[] = "head";
    hoplite_response_source_descriptor_v1_t value = descriptor(31, 0, 4);
    hoplite_response_source_state_v1_t state;
    fixture_t source = fixture(bytes, sizeof(bytes) - 1, 41, 31);

    CHECK(hoplite_response_source_init_v1(
              &state, &source, 41, &value, fixture_read, fixture_close)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(hoplite_response_source_close_v1(&state)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(source.reads == 0);
    CHECK(source.closes == 1);
    CHECK(hoplite_response_source_close_v1(&state)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(source.closes == 1);
    return 0;
}

static int
test_early_eof_fails_closed(void)
{
    static const uint8_t bytes[] = "abc";
    hoplite_response_source_descriptor_v1_t value = descriptor(43, 0, 5);
    hoplite_response_source_state_v1_t state;
    fixture_t source = fixture(bytes, sizeof(bytes) - 1, 47, 43);
    uint8_t output[8];
    size_t returned;
    uint8_t last;

    CHECK(hoplite_response_source_init_v1(
              &state, &source, 47, &value, fixture_read, fixture_close)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(returned == 3 && last == 0);
    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_ERROR);
    CHECK(returned == 0 && last == 0);
    CHECK(source.closes == 1);
    return 0;
}

static int
test_overread_fails_closed(void)
{
    static const uint8_t bytes[] = "abcd";
    hoplite_response_source_descriptor_v1_t value = descriptor(53, 0, 4);
    hoplite_response_source_state_v1_t state;
    fixture_t source = fixture(bytes, sizeof(bytes) - 1, 59, 53);
    uint8_t output[4];
    size_t returned;
    uint8_t last;

    source.overread = 1;
    CHECK(hoplite_response_source_init_v1(
              &state, &source, 59, &value, fixture_read, fixture_close)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_ERROR);
    CHECK(source.closes == 1);
    return 0;
}

static int
test_wrong_owner_and_close_failure(void)
{
    static const uint8_t bytes[] = "owner";
    hoplite_response_source_descriptor_v1_t value = descriptor(61, 0, 5);
    hoplite_response_source_state_v1_t state;
    fixture_t source = fixture(bytes, sizeof(bytes) - 1, 67, 61);
    uint8_t output[5];
    size_t returned;
    uint8_t last;

    source.expected_work = 68;
    CHECK(hoplite_response_source_init_v1(
              &state, &source, 67, &value, fixture_read, fixture_close)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_ERROR);
    CHECK(state.closed == 1);

    source = fixture(bytes, sizeof(bytes) - 1, 71, 61);
    source.fail_close = 1;
    CHECK(hoplite_response_source_init_v1(
              &state, &source, 71, &value, fixture_read, fixture_close)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(hoplite_response_source_next_v1(
              &state, output, sizeof(output), &returned, &last)
          == HOPLITE_RESPONSE_SOURCE_ERROR);
    CHECK(returned == 5 && last == 1);
    CHECK(source.closes == 1);
    CHECK(hoplite_response_source_close_v1(&state)
          == HOPLITE_RESPONSE_SOURCE_OK);
    CHECK(source.closes == 1);
    return 0;
}

int
main(void)
{
    CHECK(test_descriptor_validation() == 0);
    CHECK(test_bounded_resume_and_final_close() == 0);
    CHECK(test_head_closes_without_reading() == 0);
    CHECK(test_early_eof_fails_closed() == 0);
    CHECK(test_overread_fails_closed() == 0);
    CHECK(test_wrong_owner_and_close_failure() == 0);
    return 0;
}
