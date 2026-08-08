#include "hoplite_response_source.h"

#include <string.h>

int32_t
hoplite_response_source_descriptor_validate_v1(
    const hoplite_response_source_descriptor_v1_t *descriptor)
{
    static const uint8_t protocol[] = HOPLITE_RESPONSE_SOURCE_PROTOCOL;
    uint64_t maximum = HOPLITE_RESPONSE_SOURCE_SAFE_INTEGER_MAX;

    if (descriptor == NULL
        || descriptor->protocol == NULL
        || descriptor->protocol_len != sizeof(protocol) - 1
        || memcmp(descriptor->protocol, protocol, sizeof(protocol) - 1) != 0
        || descriptor->source_handle == 0
        || descriptor->source_handle > maximum
        || descriptor->offset > maximum
        || descriptor->length == 0
        || descriptor->length > maximum
        || descriptor->offset > maximum - descriptor->length)
    {
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }
    return HOPLITE_RESPONSE_SOURCE_OK;
}

int32_t
hoplite_response_source_init_v1(
    hoplite_response_source_state_v1_t *state,
    void *request_context,
    uint64_t work,
    const hoplite_response_source_descriptor_v1_t *descriptor,
    hoplite_response_source_read_pt read,
    hoplite_response_source_close_pt close)
{
    if (state == NULL
        || request_context == NULL
        || work == 0
        || read == NULL
        || close == NULL
        || hoplite_response_source_descriptor_validate_v1(descriptor)
               != HOPLITE_RESPONSE_SOURCE_OK)
    {
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }

    memset(state, 0, sizeof(*state));
    state->request_context = request_context;
    state->work = work;
    state->source_handle = descriptor->source_handle;
    state->offset = descriptor->offset;
    state->length = descriptor->length;
    state->cursor = descriptor->offset;
    state->remaining = descriptor->length;
    state->read = read;
    state->close = close;
    state->initialized = 1;
    return HOPLITE_RESPONSE_SOURCE_OK;
}

int32_t
hoplite_response_source_close_v1(
    hoplite_response_source_state_v1_t *state)
{
    int32_t rc;

    if (state == NULL) {
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }
    if (!state->initialized || state->closed) {
        return HOPLITE_RESPONSE_SOURCE_OK;
    }

    state->closed = 1;
    rc = state->close(state->request_context,
                      state->work,
                      state->source_handle);
    return rc == HOPLITE_RESPONSE_SOURCE_OK
        ? HOPLITE_RESPONSE_SOURCE_OK
        : HOPLITE_RESPONSE_SOURCE_ERROR;
}

int32_t
hoplite_response_source_next_v1(
    hoplite_response_source_state_v1_t *state,
    uint8_t *output,
    size_t capacity,
    size_t *returned,
    uint8_t *last)
{
    size_t allowed, read = 0;
    int32_t rc;

    if (returned != NULL) {
        *returned = 0;
    }
    if (last != NULL) {
        *last = 0;
    }
    if (state == NULL || returned == NULL || last == NULL
        || !state->initialized || state->closed)
    {
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }
    if (state->remaining == 0) {
        *last = 1;
        return HOPLITE_RESPONSE_SOURCE_DONE;
    }
    if (output == NULL || capacity == 0) {
        (void) hoplite_response_source_close_v1(state);
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }

    allowed = capacity;
    if (state->remaining < (uint64_t) allowed) {
        allowed = (size_t) state->remaining;
    }
    rc = state->read(state->request_context,
                     state->work,
                     state->source_handle,
                     output,
                     allowed,
                     &read);
    if (rc != HOPLITE_RESPONSE_SOURCE_OK || read == 0 || read > allowed) {
        (void) hoplite_response_source_close_v1(state);
        return HOPLITE_RESPONSE_SOURCE_ERROR;
    }

    state->cursor += (uint64_t) read;
    state->remaining -= (uint64_t) read;
    *returned = read;
    if (state->remaining == 0) {
        *last = 1;
        if (hoplite_response_source_close_v1(state)
            != HOPLITE_RESPONSE_SOURCE_OK)
        {
            return HOPLITE_RESPONSE_SOURCE_ERROR;
        }
    }
    return HOPLITE_RESPONSE_SOURCE_OK;
}
