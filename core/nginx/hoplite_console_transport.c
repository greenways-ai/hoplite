#include "hoplite_console_transport.h"

#include <limits.h>
#include <string.h>

#define HTA_NIL 0u
#define HTA_FALSE 1u
#define HTA_TRUE 2u
#define HTA_I64 3u
#define HTA_STRING 4u
#define HTA_BYTES 5u
#define HTA_KEYWORD 6u
#define HTA_SYMBOL 7u
#define HTA_LIST 8u
#define HTA_VECTOR 9u
#define HTA_SET 10u
#define HTA_MAP 11u
#define HTA_HANDLE 12u
#define HTA_NAMESPACE 13u
#define HTA_VAR 14u
#define HTA_F64 15u
#define HTA_ATOM 16u
#define HTA_ARRAY 17u
#define HTA_OBJECT 18u
#define HTA_CHARACTER 19u
#define HTA_BIG_INTEGER 20u
#define HTA_DECIMAL 21u
#define HTA_REGEX 22u
#define HTA_TUPLE 23u
#define HTA_CONS 24u
#define HTA_QUEUE 25u
#define HTA_ORDERED_MAP 26u
#define HTA_SORTED_MAP 27u
#define HTA_TRIE 28u
#define HTA_ORDERED_SET 29u
#define HTA_SORTED_SET 30u
#define HTA_TAGGED 31u
#define HTA_EXCEPTION_INFO 32u
#define HTA_STRUCT 33u
#define HTA_POINTER 34u
#define HTA_VAR_REF 35u
#define HTA_DEQUE 36u
#define HTA_PRIORITY_MAP 37u

#define HOPLITE_CONSOLE_FIELD_OP 0x01u
#define HOPLITE_CONSOLE_FIELD_GRANT 0x02u
#define HOPLITE_CONSOLE_FIELD_COMMAND 0x04u
#define HOPLITE_CONSOLE_FIELD_INPUT 0x08u
#define HOPLITE_CONSOLE_REQUIRED_FIELDS 0x0fu

static const uint8_t hta_magic[] = {'H', 'T', 'A', '0'};

typedef struct {
    const uint8_t *data;
    size_t len;
    size_t cursor;
} reader_t;

typedef struct {
    uint8_t *data;
    size_t len;
    size_t cursor;
} writer_t;

#include "hoplite_console_transport_read.inc"
#include "hoplite_console_transport_write.inc"

void hoplite_console_frame_reader_init(
    hoplite_console_frame_reader_t *reader,
    uint8_t *payload,
    size_t payload_capacity)
{
    if (reader == NULL) {
        return;
    }
    memset(reader, 0, sizeof(*reader));
    reader->payload = payload;
    reader->payload_capacity = payload_capacity;
    if (payload == NULL || payload_capacity == 0
        || payload_capacity > UINT32_MAX)
    {
        reader->failed = 1;
    }
}

int hoplite_console_frame_reader_feed(
    hoplite_console_frame_reader_t *reader,
    const uint8_t *input,
    size_t input_len,
    size_t *consumed)
{
    size_t amount;
    uint32_t length;

    if (consumed != NULL) {
        *consumed = 0;
    }
    if (reader == NULL || consumed == NULL
        || (input_len != 0 && input == NULL)
        || reader->failed || reader->complete)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }

    while (reader->prefix_read < HOPLITE_CONSOLE_FRAME_PREFIX_BYTES
           && *consumed < input_len)
    {
        reader->prefix[reader->prefix_read++] = input[(*consumed)++];
    }
    if (reader->prefix_read < HOPLITE_CONSOLE_FRAME_PREFIX_BYTES) {
        return HOPLITE_CONSOLE_TRANSPORT_MORE;
    }
    if (reader->payload_length == 0) {
        length = ((uint32_t) reader->prefix[0] << 24)
               | ((uint32_t) reader->prefix[1] << 16)
               | ((uint32_t) reader->prefix[2] << 8)
               | (uint32_t) reader->prefix[3];
        if (length == 0 || (size_t) length > reader->payload_capacity) {
            reader->failed = 1;
            return HOPLITE_CONSOLE_TRANSPORT_ERROR;
        }
        reader->payload_length = (size_t) length;
    }

    amount = reader->payload_length - reader->payload_read;
    if (amount > input_len - *consumed) {
        amount = input_len - *consumed;
    }
    if (amount != 0) {
        memcpy(reader->payload + reader->payload_read,
               input + *consumed,
               amount);
        reader->payload_read += amount;
        *consumed += amount;
    }
    if (reader->payload_read == reader->payload_length) {
        reader->complete = 1;
        return HOPLITE_CONSOLE_TRANSPORT_OK;
    }
    return HOPLITE_CONSOLE_TRANSPORT_MORE;
}

hoplite_console_slice_t hoplite_console_frame_reader_payload(
    const hoplite_console_frame_reader_t *reader)
{
    hoplite_console_slice_t output = {NULL, 0};
    if (reader != NULL && reader->complete && !reader->failed) {
        output.data = reader->payload;
        output.len = reader->payload_length;
    }
    return output;
}

int hoplite_console_value_validate(const uint8_t *value, size_t value_len)
{
    reader_t reader;
    if (value == NULL || value_len == 0) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    reader.data = value;
    reader.len = value_len;
    reader.cursor = 0;
    return skip_immutable_value(&reader, 0) == 0 && reader.cursor == reader.len
        ? HOPLITE_CONSOLE_TRANSPORT_OK
        : HOPLITE_CONSOLE_TRANSPORT_ERROR;
}

int hoplite_console_call_parse(
    const uint8_t *payload,
    size_t payload_len,
    hoplite_console_call_t *call)
{
    reader_t reader;
    const uint8_t *tag;
    hoplite_console_slice_t key;
    hoplite_console_slice_t value;
    hoplite_console_slice_t text;
    uint32_t count;
    uint32_t index;
    uint32_t seen = 0;

    if (payload == NULL || call == NULL || payload_len < sizeof(hta_magic)
        || memcmp(payload, hta_magic, sizeof(hta_magic)) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    memset(call, 0, sizeof(*call));
    reader.data = payload;
    reader.len = payload_len;
    reader.cursor = sizeof(hta_magic);

    if (take(&reader, 1, &tag) != 0 || tag[0] != HTA_MAP
        || read_u32(&reader, &count) != 0 || count != 4u)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    for (index = 0; index < count; index++) {
        if (read_key(&reader, &key) != 0
            || read_value_slice(&reader, &value) != 0)
        {
            return HOPLITE_CONSOLE_TRANSPORT_ERROR;
        }
        if (text_equal(key, "op")) {
            if ((seen & HOPLITE_CONSOLE_FIELD_OP) != 0
                || read_text_value(value, &text) != 0
                || !text_equal(text, "call"))
            {
                return HOPLITE_CONSOLE_TRANSPORT_ERROR;
            }
            seen |= HOPLITE_CONSOLE_FIELD_OP;
        } else if (text_equal(key, "grant")) {
            if ((seen & HOPLITE_CONSOLE_FIELD_GRANT) != 0) {
                return HOPLITE_CONSOLE_TRANSPORT_ERROR;
            }
            call->grant = value;
            seen |= HOPLITE_CONSOLE_FIELD_GRANT;
        } else if (text_equal(key, "command")) {
            if ((seen & HOPLITE_CONSOLE_FIELD_COMMAND) != 0
                || read_text_value(value, &text) != 0)
            {
                return HOPLITE_CONSOLE_TRANSPORT_ERROR;
            }
            call->command = value;
            seen |= HOPLITE_CONSOLE_FIELD_COMMAND;
        } else if (text_equal(key, "input")) {
            if ((seen & HOPLITE_CONSOLE_FIELD_INPUT) != 0) {
                return HOPLITE_CONSOLE_TRANSPORT_ERROR;
            }
            call->input = value;
            seen |= HOPLITE_CONSOLE_FIELD_INPUT;
        } else {
            return HOPLITE_CONSOLE_TRANSPORT_ERROR;
        }
    }
    return seen == HOPLITE_CONSOLE_REQUIRED_FIELDS
        && reader.cursor == reader.len
        ? HOPLITE_CONSOLE_TRANSPORT_OK
        : HOPLITE_CONSOLE_TRANSPORT_ERROR;
}

#include "hoplite_console_transport_encode.inc"
