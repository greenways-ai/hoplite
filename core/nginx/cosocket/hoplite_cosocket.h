#ifndef HOPLITE_COSOCKET_H
#define HOPLITE_COSOCKET_H

#include <ngx_config.h>
#include <ngx_core.h>

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
