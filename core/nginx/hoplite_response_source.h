#ifndef HOPLITE_RESPONSE_SOURCE_H
#define HOPLITE_RESPONSE_SOURCE_H

#include <stddef.h>
#include <stdint.h>

#define HOPLITE_RESPONSE_SOURCE_PROTOCOL "hoplite.response-source/0-alpha"
#define HOPLITE_RESPONSE_SOURCE_SAFE_INTEGER_MAX UINT64_C(9007199254740991)

#define HOPLITE_RESPONSE_SOURCE_OK 0
#define HOPLITE_RESPONSE_SOURCE_DONE 1
#define HOPLITE_RESPONSE_SOURCE_ERROR (-1)

typedef int32_t (*hoplite_response_source_read_pt)(
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

typedef int32_t (*hoplite_response_source_close_pt)(
    void *request_context,
    uint64_t work,
    uint64_t source_handle);

typedef struct {
    const uint8_t *protocol;
    size_t protocol_len;
    const uint8_t *service;
    size_t service_len;
    uint64_t source_handle;
    uint64_t offset;
    uint64_t length;
} hoplite_response_source_descriptor_v1_t;

typedef struct {
    void *request_context;
    uint64_t work;
    uint64_t source_handle;
    uint64_t offset;
    uint64_t length;
    uint64_t cursor;
    uint64_t remaining;
    hoplite_response_source_read_pt read;
    hoplite_response_source_close_pt close;
    uint8_t initialized;
    uint8_t closed;
} hoplite_response_source_state_v1_t;

/*
 * Validate the closed portable descriptor. The protocol bytes are compared
 * immediately and are never retained by the state machine.
 */
int32_t hoplite_response_source_descriptor_validate_v1(
    const hoplite_response_source_descriptor_v1_t *descriptor);

/*
 * Bind one validated descriptor to the exact opaque request identity and work
 * that created its source handle.
 */
int32_t hoplite_response_source_init_v1(
    hoplite_response_source_state_v1_t *state,
    void *request_context,
    uint64_t work,
    const hoplite_response_source_descriptor_v1_t *descriptor,
    hoplite_response_source_read_pt read,
    hoplite_response_source_close_pt close);

/*
 * Read one bounded chunk. The callback can never receive a capacity greater
 * than the descriptor's remaining length. A zero read before completion and a
 * callback over-read both fail closed. The source is closed after the final
 * successful read.
 */
int32_t hoplite_response_source_next_v1(
    hoplite_response_source_state_v1_t *state,
    uint8_t *output,
    size_t capacity,
    size_t *returned,
    uint8_t *last);

/*
 * Close at most once. Calling close for an uninitialized or already-closed
 * state is a successful no-op, which makes request cleanup deterministic.
 */
int32_t hoplite_response_source_close_v1(
    hoplite_response_source_state_v1_t *state);

#endif /* HOPLITE_RESPONSE_SOURCE_H */
