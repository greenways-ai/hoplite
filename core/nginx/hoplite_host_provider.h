#ifndef HOPLITE_HOST_PROVIDER_H
#define HOPLITE_HOST_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#include "hoplite_host_registry.h"

#define HOPLITE_HOST_PROVIDER_ABI_VERSION 1u

#define HOPLITE_HOST_PROVIDER_OK 0
#define HOPLITE_HOST_PROVIDER_PENDING 1
#define HOPLITE_HOST_PROVIDER_ERROR (-1)

#define HOPLITE_HOST_PROVIDER_REGISTER_OK 0
#define HOPLITE_HOST_PROVIDER_REGISTER_INVALID 1
#define HOPLITE_HOST_PROVIDER_REGISTER_DUPLICATE 2
#define HOPLITE_HOST_PROVIDER_REGISTER_FULL 3
#define HOPLITE_HOST_PROVIDER_REGISTER_ABI_MISMATCH 4

#define HOPLITE_HOST_PROVIDER_REQUEST_BODY 0x01u
#define HOPLITE_HOST_PROVIDER_RESPONSE_BODY 0x02u

#define HOPLITE_HOST_RESOURCE_OK 0
#define HOPLITE_HOST_RESOURCE_ERROR (-1)

typedef int32_t (*hoplite_host_complete_v1_pt)(
    void *context,
    const uint8_t *hta,
    size_t hta_len);

typedef struct {
    void *context;
    hoplite_host_complete_v1_pt succeed;
    hoplite_host_complete_v1_pt fail;
} hoplite_host_completer_v1_t;

typedef struct {
    uint32_t abi_version;
    void *request_context;
    uint64_t work;
    uint64_t call;
    hoplite_host_service_t operation;
    /* Exact standalone HTA1 frame for the Hara call arguments. */
    hoplite_host_service_t arguments_hta;
    hoplite_host_completer_v1_t completer;
} hoplite_host_call_v1_t;

typedef int32_t (*hoplite_host_provider_invoke_v1_pt)(
    const hoplite_host_call_v1_t *call);
typedef void (*hoplite_host_provider_cancel_v1_pt)(
    void *request_context);
typedef int32_t (*hoplite_host_provider_response_read_v1_pt)(
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);
typedef int32_t (*hoplite_host_provider_response_close_v1_pt)(
    void *request_context,
    uint64_t work,
    uint64_t source_handle);
typedef size_t (*hoplite_host_provider_release_work_v1_pt)(uint64_t work);

typedef struct {
    uint32_t abi_version;
    hoplite_host_service_t service;
    hoplite_host_provider_invoke_v1_pt invoke;
    hoplite_host_provider_cancel_v1_pt cancel;
    uint32_t capabilities;
    hoplite_host_provider_response_read_v1_pt response_read;
    hoplite_host_provider_response_close_v1_pt response_close;
    hoplite_host_provider_release_work_v1_pt release_work;
} hoplite_host_provider_v1_t;

/*
 * Register one immutable provider descriptor for the current Nginx worker.
 * Registration is exact, case-sensitive and valid only during trusted worker
 * startup. The descriptor, callback pointers and service bytes must outlive the
 * worker. Application values never reach this function.
 */
int32_t hoplite_host_provider_register_v1(
    const hoplite_host_provider_v1_t *provider);

/* Native dispatch lookup; request values cannot mutate the registry. */
const hoplite_host_provider_v1_t *hoplite_host_provider_find_v1(
    hoplite_host_service_t service);

/*
 * Read or finish one opaque request-body handle through the exact request and
 * work scope carried by a provider call.
 *
 * Access succeeds only while the provider is actively invoked or retained for
 * that request, and only when its descriptor declares
 * HOPLITE_HOST_PROVIDER_REQUEST_BODY. A numeric handle alone grants no access.
 * The host verifies request_context, work and handle before a native body
 * callback runs.
 *
 * read returns HOPLITE_HOST_RESOURCE_OK for a successful bounded read. EOF is
 * represented by *returned == 0. Every other condition returns
 * HOPLITE_HOST_RESOURCE_ERROR and sets *returned to zero when it is available.
 * finish consumes and closes the handle exactly once for the owning work.
 */
int32_t hoplite_host_request_body_read_v1(
    void *request_context,
    uint64_t work,
    uint64_t handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

int32_t hoplite_host_request_body_finish_v1(
    void *request_context,
    uint64_t work,
    uint64_t handle);

#endif
