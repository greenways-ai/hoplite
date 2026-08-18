#ifndef HOPLITE_COSOCKET_H
#define HOPLITE_COSOCKET_H

#include <ngx_config.h>
#include <ngx_core.h>

/* Register the worker-local hoplite.socket native provider. */
ngx_int_t hoplite_cosocket_register(ngx_cycle_t *cycle);

/* Close every remaining worker-local cosocket during worker shutdown. */
void hoplite_cosocket_worker_exit(void);

#endif
