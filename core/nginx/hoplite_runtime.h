#ifndef HOPLITE_RUNTIME_H
#define HOPLITE_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#include "../abi/data-plane-ffi/include/hoplite_data_plane.h"

typedef struct hoplite_runtime hoplite_runtime_t;

typedef struct {
    uint8_t *data;
    size_t len;
} hoplite_buffer_t;

typedef struct {
    const uint8_t *data;
    size_t len;
} hoplite_slice_t;

typedef int (*hoplite_header_at_fn)(void *context, size_t index,
                                    hoplite_slice_t *name,
                                    hoplite_slice_t *value);

typedef struct {
    void *context;
    hoplite_slice_t method;
    hoplite_slice_t uri;
    hoplite_slice_t path;
    hoplite_slice_t query_string;
    hoplite_slice_t remote_address;
    size_t header_count;
    hoplite_header_at_fn header_at;
} hoplite_request_v2_t;

/*
 * V3 preserves the complete V2 request metadata layout and adds one optional
 * native body descriptor plus server-selected limits. A null body pointer
 * requires every limit field to be zero.
 *
 * After the runtime, request and outcome pointers pass their non-null preflight,
 * a non-null body transfers exclusive descriptor lifecycle ownership to the
 * worker runtime, including later validation failure. The caller must not reuse
 * or close the descriptor after that transfer. If top-level pointer preflight
 * fails, ownership remains with the caller.
 */
typedef struct {
    hoplite_request_v2_t request;
    const hoplite_request_body_v1 *body;
    uint64_t max_body_bytes;
    size_t max_chunk_bytes;
    uint32_t require_declared_length;
} hoplite_request_v3_t;

typedef struct {
    uint32_t kind;
    uint64_t id;
} hoplite_outcome_v2_t;

/*
 * Returns the highest runtime ABI version supported by this library. ABI
 * additions preserve earlier versioned symbols, so a V2-only host accepts any
 * return value greater than or equal to 2 rather than requiring exact equality.
 */
uint32_t hoplite_abi_version(void);
hoplite_runtime_t *hoplite_runtime_new(void);
void hoplite_runtime_free(hoplite_runtime_t *runtime);
int hoplite_bootstrap_modules(hoplite_runtime_t *runtime,
                              const uint8_t *source,
                              size_t source_len);
/* Transactionally load a deterministic alpha HBX application bundle. */
int hoplite_bootstrap_bytecode(hoplite_runtime_t *runtime,
                               const uint8_t *bundle,
                               size_t bundle_len);

uint64_t hoplite_handler_prepare(hoplite_runtime_t *runtime,
                                 const uint8_t *function,
                                 size_t function_len);

uint64_t hoplite_work_call(hoplite_runtime_t *runtime,
                           uint64_t handler,
                           const uint8_t *input,
                           size_t input_len);

int hoplite_apps_prepare(hoplite_runtime_t *runtime,
                         const uint8_t *manifest,
                         size_t manifest_len);
uint64_t hoplite_app_call(hoplite_runtime_t *runtime,
                          uint64_t app,
                          const uint8_t *input,
                          size_t input_len);
int hoplite_app_invoke_v2(hoplite_runtime_t *runtime,
                          uint64_t app,
                          const hoplite_request_v2_t *request,
                          hoplite_outcome_v2_t *outcome);
int hoplite_handler_invoke_v2(hoplite_runtime_t *runtime,
                              uint64_t handler,
                              uint32_t adapter,
                              const hoplite_request_v2_t *request,
                              hoplite_outcome_v2_t *outcome);
int hoplite_app_invoke_v3(hoplite_runtime_t *runtime,
                          uint64_t app,
                          const hoplite_request_v3_t *request,
                          hoplite_outcome_v2_t *outcome);
int hoplite_handler_invoke_v3(hoplite_runtime_t *runtime,
                              uint64_t handler,
                              uint32_t adapter,
                              const hoplite_request_v3_t *request,
                              hoplite_outcome_v2_t *outcome);

/*
 * Resolve one request body through the owning suspended work. The trusted host
 * must pass the work id carried by the corresponding Hoplite host-call event.
 * A handle associated with another live work or a closed work fails before a
 * native callback runs.
 */
int hoplite_request_body_read_v3(hoplite_runtime_t *runtime,
                                 uint64_t work,
                                 uint64_t handle,
                                 uint8_t *output,
                                 size_t capacity,
                                 size_t *returned);
int hoplite_request_body_finish_v3(hoplite_runtime_t *runtime,
                                   uint64_t work,
                                   uint64_t handle);

int hoplite_response_status_v2(hoplite_runtime_t *runtime,
                               uint64_t response,
                               uint16_t *status);
int hoplite_response_body_v2(hoplite_runtime_t *runtime,
                             uint64_t response,
                             hoplite_slice_t *body);
size_t hoplite_response_header_count_v2(hoplite_runtime_t *runtime,
                                        uint64_t response);
int hoplite_response_header_at_v2(hoplite_runtime_t *runtime,
                                  uint64_t response,
                                  size_t index,
                                  hoplite_slice_t *name,
                                  hoplite_slice_t *value);
int hoplite_response_close_v2(hoplite_runtime_t *runtime,
                              uint64_t response);

uint64_t hoplite_work_start(hoplite_runtime_t *runtime,
                            const uint8_t *function,
                            size_t function_len,
                            const uint8_t *input,
                            size_t input_len);
int hoplite_handler_close(hoplite_runtime_t *runtime, uint64_t handler);

size_t hoplite_work_poll(hoplite_runtime_t *runtime);
int hoplite_work_next_event(hoplite_runtime_t *runtime, hoplite_buffer_t *output);
void hoplite_buffer_free(uint8_t *data, size_t len);

int hoplite_work_send(hoplite_runtime_t *runtime,
                      uint64_t work,
                      const uint8_t *message,
                      size_t message_len);

int hoplite_call_resolve(hoplite_runtime_t *runtime,
                         uint64_t call,
                         const uint8_t *payload,
                         size_t payload_len);

int hoplite_call_reject(hoplite_runtime_t *runtime,
                        uint64_t call,
                        const uint8_t *payload,
                        size_t payload_len);

int hoplite_work_cancel(hoplite_runtime_t *runtime, uint64_t work);
int hoplite_work_close(hoplite_runtime_t *runtime, uint64_t work);

#endif
