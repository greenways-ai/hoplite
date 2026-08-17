#ifndef HOPLITE_CONSOLE_WORKER_H
#define HOPLITE_CONSOLE_WORKER_H

#include <stddef.h>
#include <stdint.h>

#include "hoplite_console_transport.h"

#ifdef __cplusplus
extern "C" {
#endif

#define HOPLITE_CONSOLE_WORKER_OK 0
#define HOPLITE_CONSOLE_WORKER_MORE 1
#define HOPLITE_CONSOLE_WORKER_STARTED 2
#define HOPLITE_CONSOLE_WORKER_OUTPUT 3
#define HOPLITE_CONSOLE_WORKER_DONE 4
#define HOPLITE_CONSOLE_WORKER_FATAL (-1)

typedef enum {
    HOPLITE_CONSOLE_WORKER_READING = 0,
    HOPLITE_CONSOLE_WORKER_RUNNING = 1,
    HOPLITE_CONSOLE_WORKER_WRITING = 2,
    HOPLITE_CONSOLE_WORKER_FINISHED = 3,
    HOPLITE_CONSOLE_WORKER_BROKEN = 4
} hoplite_console_worker_phase_t;

typedef uint64_t (*hoplite_console_worker_call_pt)(
    void *context,
    uint64_t app,
    const uint8_t *input,
    size_t input_len);

typedef int (*hoplite_console_worker_work_pt)(
    void *context,
    uint64_t work);

typedef struct {
    hoplite_console_worker_call_pt call;
    hoplite_console_worker_work_pt cancel;
    hoplite_console_worker_work_pt close;
} hoplite_console_worker_ops_t;

typedef struct {
    size_t request_bytes;
    size_t result_bytes;
} hoplite_console_worker_limits_t;

typedef struct {
    void *runtime_context;
    uint64_t app;
    uint64_t work;
    hoplite_console_worker_ops_t ops;
    hoplite_console_worker_limits_t limits;

    hoplite_console_frame_reader_t reader;
    uint8_t *request_storage;
    size_t request_capacity;
    uint8_t *application_storage;
    size_t application_capacity;
    size_t application_length;
    uint8_t *response_storage;
    size_t response_capacity;
    size_t response_length;
    size_t response_offset;

    hoplite_console_worker_phase_t phase;
    uint8_t work_closed;
    uint8_t cancel_attempted;
} hoplite_console_worker_t;

int hoplite_console_worker_init(
    hoplite_console_worker_t *worker,
    void *runtime_context,
    uint64_t app,
    hoplite_console_worker_ops_t ops,
    hoplite_console_worker_limits_t limits,
    uint8_t *request_storage,
    size_t request_capacity,
    uint8_t *application_storage,
    size_t application_capacity,
    uint8_t *response_storage,
    size_t response_capacity);

/*
 * Consume connection bytes until one exact broker call is complete. Once the
 * application work has started, this function rejects all further input.
 */
int hoplite_console_worker_feed(
    hoplite_console_worker_t *worker,
    const uint8_t *input,
    size_t input_len,
    size_t *consumed);

/* Complete a running work with one bare immutable HTA0 value. */
int hoplite_console_worker_complete(
    hoplite_console_worker_t *worker,
    const uint8_t *value,
    size_t value_len);

/* Complete a running work with a redacted structured failure. */
int hoplite_console_worker_fail(
    hoplite_console_worker_t *worker,
    const uint8_t *code,
    size_t code_len,
    const uint8_t *message,
    size_t message_len);

/*
 * A console command is never allowed to request an application host call. The
 * running work is cancelled and closed before a bounded failure is exposed.
 */
int hoplite_console_worker_forbid_host_call(
    hoplite_console_worker_t *worker);

/* Cancel and close a running work, then expose a bounded timeout failure. */
int hoplite_console_worker_timeout(
    hoplite_console_worker_t *worker);

/*
 * Disconnect/reload cleanup. A cancellation or close failure is fatal so the
 * embedding worker can terminate rather than retain a suspended application.
 */
int hoplite_console_worker_abort(
    hoplite_console_worker_t *worker);

int hoplite_console_worker_output(
    const hoplite_console_worker_t *worker,
    hoplite_console_slice_t *output);

int hoplite_console_worker_consume_output(
    hoplite_console_worker_t *worker,
    size_t amount);

uint64_t hoplite_console_worker_work(
    const hoplite_console_worker_t *worker);

hoplite_console_worker_phase_t hoplite_console_worker_phase(
    const hoplite_console_worker_t *worker);

#ifdef __cplusplus
}
#endif

#endif
