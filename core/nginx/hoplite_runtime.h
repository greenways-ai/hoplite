#ifndef HOPLITE_RUNTIME_H
#define HOPLITE_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#include "../abi/data-plane-ffi/include/hoplite_data_plane.h"

typedef struct hoplite_runtime hoplite_runtime_t;
typedef struct hoplite_rtc_engine hoplite_rtc_engine_t;

typedef struct {
    uint8_t *data;
    size_t len;
} hoplite_buffer_t;

typedef struct {
    uint32_t kind;
    uint64_t timeout_millis;
    hoplite_buffer_t source;
    hoplite_buffer_t destination;
    hoplite_buffer_t payload;
    uint32_t binary;
} hoplite_rtc_poll_t;

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

/*
 * Runtime ABI 5 introduces V4, which composes V3 with an optional,
 * borrowed raw-request descriptor. The callback accepts only the closed field
 * identifiers below. It does not accept
 * variable names, Nginx directives, locations, upstreams, paths or handles.
 *
 * Callback results are borrowed UTF-8 bytes valid for the callback duration:
 * HOPLITE_RAW_FIELD_OK, HOPLITE_RAW_FIELD_UNAVAILABLE or
 * HOPLITE_RAW_FIELD_ERROR.
 *
 * A non-null raw descriptor and its context remain valid through the active
 * request invocation, including suspension. The runtime copies the descriptor,
 * not the pointed-to Nginx request. Raw validation precedes V3 body ownership
 * transfer, so a raw-validation failure leaves a body descriptor with the
 * caller.
 */
#define HOPLITE_RAW_FIELD_OK 0
#define HOPLITE_RAW_FIELD_UNAVAILABLE 1
#define HOPLITE_RAW_FIELD_ERROR 2

#define HOPLITE_RAW_FIELD_SCHEME 1u
#define HOPLITE_RAW_FIELD_SERVER_PROTOCOL 2u
#define HOPLITE_RAW_FIELD_HOST 3u
#define HOPLITE_RAW_FIELD_SERVER_NAME 4u
#define HOPLITE_RAW_FIELD_SERVER_ADDRESS 5u
#define HOPLITE_RAW_FIELD_SERVER_PORT 6u
#define HOPLITE_RAW_FIELD_REMOTE_PORT 7u
#define HOPLITE_RAW_FIELD_REQUEST_ID 8u
#define HOPLITE_RAW_FIELD_CONNECTION_ID 9u
#define HOPLITE_RAW_FIELD_CONNECTION_REQUESTS 10u
#define HOPLITE_RAW_FIELD_REQUEST_TIME 11u
#define HOPLITE_RAW_FIELD_REQUEST_LENGTH 12u
#define HOPLITE_RAW_FIELD_CONTENT_LENGTH 13u

typedef int32_t (*hoplite_raw_field_fn)(void *context,
                                        uint32_t field,
                                        hoplite_slice_t *value);

typedef struct {
    void *context;
    hoplite_raw_field_fn field;
} hoplite_raw_request_v1_t;

typedef struct {
    hoplite_request_v3_t request;
    const hoplite_raw_request_v1_t *raw;
} hoplite_request_v4_t;

typedef struct {
    uint32_t kind;
    uint64_t id;
} hoplite_outcome_v2_t;

typedef void (*hoplite_startup_diagnostic_fn)(void *context,
                                              const uint8_t *diagnostic,
                                              size_t diagnostic_len);

/*
 * Returns the highest runtime ABI version supported by this library. ABI
 * additions preserve earlier versioned symbols, so a V2-only host accepts any
 * return value greater than or equal to 2 rather than requiring exact equality.
 * A host constructing the V4 raw-request descriptor requires ABI 5 or newer.
 */
uint32_t hoplite_abi_version(void);
hoplite_runtime_t *hoplite_runtime_new(void);
void hoplite_runtime_free(hoplite_runtime_t *runtime);
/* Development/full-embedding library only; absent from the production link. */
int hoplite_bootstrap_modules(hoplite_runtime_t *runtime,
                              const uint8_t *source,
                              size_t source_len);
/*
 * Transactionally load a Hara HBX0 bundle. This lower-level compatibility
 * entry point does not bind or prepare a Hoplite route manifest.
 */
int hoplite_bootstrap_bytecode(hoplite_runtime_t *runtime,
                               const uint8_t *bundle,
                               size_t bundle_len);

/*
 * The `_v1` suffix on these exported bootstrap symbols identifies the first C
 * function shape. It is independent of the alpha document epoch below.
 *
 * Validate one HAB0 bundle against the exact route manifest, load its embedded
 * HBX0 modules, and prepare every app route as one transactional startup.
 */
int hoplite_bootstrap_application_v1(hoplite_runtime_t *runtime,
                                     const uint8_t *bundle,
                                     size_t bundle_len,
                                     const uint8_t *manifest,
                                     size_t manifest_len);

/*
 * ABI V4 diagnostic bootstrap. The callback receives one complete UTF-8 JSON
 * document per ordered stage. Bytes are valid only for the callback duration.
 */
int hoplite_bootstrap_application_v2(
    hoplite_runtime_t *runtime,
    const uint8_t *bundle,
    size_t bundle_len,
    const uint8_t *manifest,
    size_t manifest_len,
    hoplite_startup_diagnostic_fn diagnostic,
    void *diagnostic_context);

/*
 * Read the configured HAB0 and manifest as bounded regular files,
 * then perform the same combined transactional startup.
 */
int hoplite_bootstrap_application_files_v1(
    hoplite_runtime_t *runtime,
    const uint8_t *bundle_path,
    size_t bundle_path_len,
    const uint8_t *manifest_path,
    size_t manifest_path_len);
int hoplite_bootstrap_application_files_v2(
    hoplite_runtime_t *runtime,
    const uint8_t *bundle_path,
    size_t bundle_path_len,
    const uint8_t *manifest_path,
    size_t manifest_path_len,
    hoplite_startup_diagnostic_fn diagnostic,
    void *diagnostic_context);

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
/*
 * Invoke the one console handler selected from the immutable application
 * manifest. The caller supplies only app identity and HTA input; no source,
 * symbol, function name, or handler identifier crosses this boundary.
 */
uint64_t hoplite_app_console_call(hoplite_runtime_t *runtime,
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
int hoplite_app_invoke_v4(hoplite_runtime_t *runtime,
                          uint64_t app,
                          const hoplite_request_v4_t *request,
                          hoplite_outcome_v2_t *outcome);
int hoplite_handler_invoke_v4(hoplite_runtime_t *runtime,
                              uint64_t handler,
                              uint32_t adapter,
                              const hoplite_request_v4_t *request,
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
/* Body kind: 0 buffered, 1 worker-local Hara Stream, -1 unknown response. */
int hoplite_response_body_kind_v3(hoplite_runtime_t *runtime,
                                  uint64_t response);
/* Stream pull: 0 chunk, 1 pending, 2 EOF, -1 error. */
int hoplite_response_stream_next_v3(hoplite_runtime_t *runtime,
                                    uint64_t response,
                                    hoplite_slice_t *chunk);
size_t hoplite_response_header_count_v2(hoplite_runtime_t *runtime,
                                        uint64_t response);
int hoplite_response_header_at_v2(hoplite_runtime_t *runtime,
                                  uint64_t response,
                                  size_t index,
                                  hoplite_slice_t *name,
                                  hoplite_slice_t *value);
int hoplite_response_close_v2(hoplite_runtime_t *runtime,
                              uint64_t response);

/* Development/full-embedding library only; absent from the production link. */
uint64_t hoplite_work_start(hoplite_runtime_t *runtime,
                            const uint8_t *function,
                            size_t function_len,
                            const uint8_t *input,
                            size_t input_len);
int hoplite_handler_close(hoplite_runtime_t *runtime, uint64_t handler);

size_t hoplite_work_poll(hoplite_runtime_t *runtime);
int hoplite_work_next_event(hoplite_runtime_t *runtime, hoplite_buffer_t *output);
void hoplite_buffer_free(uint8_t *data, size_t len);

/*
 * Worker-local Sans-I/O WebRTC engine. The caller owns the opaque engine and
 * drives its socket/timer lifecycle; returned buffers use hoplite_buffer_free.
 */
hoplite_rtc_engine_t *hoplite_rtc_engine_new(const uint8_t *label,
                                             size_t label_len,
                                             size_t max_message_bytes);
void hoplite_rtc_engine_free(hoplite_rtc_engine_t *engine);
int hoplite_rtc_add_local_udp_candidate(hoplite_rtc_engine_t *engine,
                                        const uint8_t *address,
                                        size_t address_len);
int hoplite_rtc_accept_offer(hoplite_rtc_engine_t *engine,
                             const uint8_t *offer,
                             size_t offer_len,
                             hoplite_buffer_t *answer);
int hoplite_rtc_create_offer(hoplite_rtc_engine_t *engine,
                             hoplite_buffer_t *offer);
int hoplite_rtc_accept_answer(hoplite_rtc_engine_t *engine,
                              const uint8_t *answer,
                              size_t answer_len);
int hoplite_rtc_send(hoplite_rtc_engine_t *engine,
                     const uint8_t *message,
                     size_t message_len);
int hoplite_rtc_handle_timeout(hoplite_rtc_engine_t *engine);
int hoplite_rtc_handle_udp(hoplite_rtc_engine_t *engine,
                           const uint8_t *source,
                           size_t source_len,
                           const uint8_t *destination,
                           size_t destination_len,
                           const uint8_t *contents,
                           size_t contents_len);
/* kind: 0 timeout, 1 UDP transmit, 2 channel data, 3 connected, 4 closed, 5 other. */
int hoplite_rtc_poll(hoplite_rtc_engine_t *engine,
                     hoplite_rtc_poll_t *output);

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
