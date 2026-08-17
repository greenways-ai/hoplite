#ifndef HOPLITE_CONSOLE_TRANSPORT_H
#define HOPLITE_CONSOLE_TRANSPORT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HOPLITE_CONSOLE_TRANSPORT_OK 0
#define HOPLITE_CONSOLE_TRANSPORT_MORE 1
#define HOPLITE_CONSOLE_TRANSPORT_ERROR (-1)

#define HOPLITE_CONSOLE_FRAME_PREFIX_BYTES 4u
#define HOPLITE_CONSOLE_DEFAULT_MAXIMUM_BYTES (1024u * 1024u)
#define HOPLITE_CONSOLE_MAXIMUM_DEPTH 256u

typedef struct {
    const uint8_t *data;
    size_t len;
} hoplite_console_slice_t;

typedef struct {
    uint8_t prefix[HOPLITE_CONSOLE_FRAME_PREFIX_BYTES];
    size_t prefix_read;
    uint8_t *payload;
    size_t payload_capacity;
    size_t payload_length;
    size_t payload_read;
    uint8_t complete;
    uint8_t failed;
} hoplite_console_frame_reader_t;

typedef struct {
    hoplite_console_slice_t grant;
    hoplite_console_slice_t command;
    hoplite_console_slice_t input;
} hoplite_console_call_t;

void hoplite_console_frame_reader_init(
    hoplite_console_frame_reader_t *reader,
    uint8_t *payload,
    size_t payload_capacity);

int hoplite_console_frame_reader_feed(
    hoplite_console_frame_reader_t *reader,
    const uint8_t *input,
    size_t input_len,
    size_t *consumed);

hoplite_console_slice_t hoplite_console_frame_reader_payload(
    const hoplite_console_frame_reader_t *reader);

/* Validate one bare immutable HTA0 value (without the HTA0 magic). */
int hoplite_console_value_validate(
    const uint8_t *value,
    size_t value_len);

/* Parse exactly {op, grant, command, input}; all values must be immutable HTA0. */
int hoplite_console_call_parse(
    const uint8_t *payload,
    size_t payload_len,
    hoplite_console_call_t *call);

size_t hoplite_console_call_encoded_size(const hoplite_console_call_t *call);

/* Encode the closed {grant, command, input} HTA0 envelope. */
int hoplite_console_call_encode(
    const hoplite_console_call_t *call,
    uint8_t *output,
    size_t capacity,
    size_t *written);

size_t hoplite_console_success_encoded_size(size_t value_len);

/* Wrap one bare immutable HTA0 value as {:ok true :value value}. */
int hoplite_console_success_encode(
    const uint8_t *value,
    size_t value_len,
    uint8_t *output,
    size_t capacity,
    size_t *written);

size_t hoplite_console_failure_encoded_size(
    size_t code_len,
    size_t message_len);

int hoplite_console_failure_encode(
    const uint8_t *code,
    size_t code_len,
    const uint8_t *message,
    size_t message_len,
    uint8_t *output,
    size_t capacity,
    size_t *written);

int hoplite_console_frame_encode(
    const uint8_t *payload,
    size_t payload_len,
    size_t maximum_payload_bytes,
    uint8_t *output,
    size_t capacity,
    size_t *written);

#ifdef __cplusplus
}
#endif

#endif
