#ifndef HOPLITE_VALUE_HOST_PROVIDER_H
#define HOPLITE_VALUE_HOST_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#define HOPLITE_VALUE_HOST_PROVIDER_OK 0
#define HOPLITE_VALUE_HOST_PROVIDER_DISABLED 1
#define HOPLITE_VALUE_HOST_PROVIDER_ERROR (-1)

int32_t hoplite_value_host_provider_register_filesystem_v1(
    const uint8_t *root,
    size_t root_len,
    size_t max_frame_bytes);

int32_t hoplite_value_host_provider_init_process_v1(void);

void hoplite_value_host_provider_exit_process_v1(void);

#endif
