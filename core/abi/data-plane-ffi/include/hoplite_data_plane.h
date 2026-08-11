#ifndef HOPLITE_DATA_PLANE_H
#define HOPLITE_DATA_PLANE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HOPLITE_DATA_PLANE_ABI_VERSION 0u
#define HOPLITE_CALLBACK_OK 0

/*
 * The native host creates each request- or response-scoped descriptor and
 * guarantees that its context and callbacks remain valid. An ABI operation
 * that accepts a descriptor may transfer exclusive lifecycle ownership; after
 * such a transfer the caller must not reuse the descriptor or invoke close.
 * The Rust bridge never interprets context as a path, URL, file descriptor,
 * credential, or portable application value.
 */
typedef int32_t (*hoplite_read_v1)(
    void *context,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

typedef int32_t (*hoplite_read_at_v1)(
    void *context,
    uint64_t offset,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

typedef void (*hoplite_close_v1)(void *context);

typedef struct hoplite_request_body_v1 {
    void *context;
    uint64_t declared_length;
    /* 0 = unknown; 1 = declared_length is authoritative. */
    uint32_t has_declared_length;
    hoplite_read_v1 read;
    hoplite_close_v1 close;
} hoplite_request_body_v1;

typedef struct hoplite_response_body_v1 {
    void *context;
    uint64_t length;
    hoplite_read_at_v1 read_at;
    hoplite_close_v1 close;
} hoplite_response_body_v1;

#ifdef __cplusplus
}
#endif

#endif /* HOPLITE_DATA_PLANE_H */
