#ifndef HOPLITE_COSOCKET_H
#define HOPLITE_COSOCKET_H

#include <ngx_config.h>
#include <ngx_core.h>

/*
 * Internal codes for every ordinary hoplite.socket failure. The mapping to
 * text is owned by the provider so native errno text never reaches Hara.
 */
typedef enum {
    HOPLITE_COSOCKET_ERROR_TIMEOUT = 0,
    HOPLITE_COSOCKET_ERROR_CLOSED,
    HOPLITE_COSOCKET_ERROR_CONNECTION_REFUSED,
    HOPLITE_COSOCKET_ERROR_CONNECTION_RESET,
    HOPLITE_COSOCKET_ERROR_NETWORK_UNREACHABLE,
    HOPLITE_COSOCKET_ERROR_HOST_UNREACHABLE,
    HOPLITE_COSOCKET_ERROR_SOCKET_ERROR,
    HOPLITE_COSOCKET_ERROR_CONNECT_FAILED,
    HOPLITE_COSOCKET_ERROR_SEND_FAILED,
    HOPLITE_COSOCKET_ERROR_RECEIVE_FAILED,
    HOPLITE_COSOCKET_ERROR_SHUTDOWN_FAILED,
    HOPLITE_COSOCKET_ERROR_SOCKET_OPTION_FAILED,
    HOPLITE_COSOCKET_ERROR_HOST_NOT_FOUND,
    HOPLITE_COSOCKET_ERROR_RESOLVER_NOT_CONFIGURED,
    HOPLITE_COSOCKET_ERROR_RESOLVER_UNAVAILABLE,
    HOPLITE_COSOCKET_ERROR_RESOLVER_REFUSED,
    HOPLITE_COSOCKET_ERROR_RESOLVER_FAILURE,
    HOPLITE_COSOCKET_ERROR_NAME_RESOLUTION_FAILED,
    HOPLITE_COSOCKET_ERROR_SOCKET_BUSY_READING,
    HOPLITE_COSOCKET_ERROR_SOCKET_BUSY_WRITING,
    HOPLITE_COSOCKET_ERROR_POOL_CAPACITY_UNAVAILABLE,
    HOPLITE_COSOCKET_ERROR_POOL_BACKLOG_OVERFLOW,
    HOPLITE_COSOCKET_ERROR_POOL_WAIT_TIMEOUT,
    HOPLITE_COSOCKET_ERROR_CONNECTION_DUBIOUS,
    HOPLITE_COSOCKET_ERROR_ALREADY_CONNECTED,
    HOPLITE_COSOCKET_ERROR_CONNECTION_HAS_NO_POOL_IDENTITY,
    HOPLITE_COSOCKET_ERROR_SOCKET_OPTIONS_PREVENT_SAFE_POOLING
} hoplite_cosocket_error_t;

typedef enum {
    HOPLITE_COSOCKET_LIFECYCLE_ACTIVE = 0,
    HOPLITE_COSOCKET_LIFECYCLE_CANCELLED,
    HOPLITE_COSOCKET_LIFECYCLE_CLIENT_ABORTED
} hoplite_cosocket_lifecycle_t;

#ifdef HOPLITE_COSOCKET_BASE_IMPLEMENTATION

/* Internal registration used by the directional provider composition. */
ngx_int_t hoplite_cosocket_register(ngx_cycle_t *cycle);

/* Internal worker teardown used after directional state has been drained. */
void hoplite_cosocket_worker_exit(void);

#else

/* Register the worker-local hoplite.socket provider with directional I/O. */
ngx_int_t hoplite_cosocket_directional_register(ngx_cycle_t *cycle);

/* Drain directional state and every remaining worker-local cosocket. */
void hoplite_cosocket_directional_worker_exit(void);

#define hoplite_cosocket_register hoplite_cosocket_directional_register
#define hoplite_cosocket_worker_exit hoplite_cosocket_directional_worker_exit

#endif

#endif
