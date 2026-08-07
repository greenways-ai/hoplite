#ifndef HOPLITE_HOST_REGISTRY_H
#define HOPLITE_HOST_REGISTRY_H

#include <stddef.h>
#include <stdint.h>
#include <string.h>

#define HOPLITE_HOST_REGISTRY_CAPACITY 8u

typedef struct {
    const uint8_t *data;
    size_t len;
} hoplite_host_service_t;

typedef struct {
    hoplite_host_service_t service;
    const void *provider;
} hoplite_host_registry_entry_t;

typedef struct {
    hoplite_host_registry_entry_t entries[HOPLITE_HOST_REGISTRY_CAPACITY];
    size_t count;
} hoplite_host_registry_t;

enum {
    HOPLITE_HOST_REGISTRY_OK = 0,
    HOPLITE_HOST_REGISTRY_INVALID = 1,
    HOPLITE_HOST_REGISTRY_DUPLICATE = 2,
    HOPLITE_HOST_REGISTRY_FULL = 3
};

static inline int
hoplite_host_service_equal(hoplite_host_service_t left,
                           hoplite_host_service_t right)
{
    return left.len == right.len
        && left.len != 0
        && memcmp(left.data, right.data, left.len) == 0;
}

static inline void
hoplite_host_registry_init(hoplite_host_registry_t *registry)
{
    if (registry == NULL) {
        return;
    }
    memset(registry, 0, sizeof(*registry));
}

/*
 * Service bytes and the provider object must outlive the registry. The Nginx
 * module registers static providers once during worker initialization, so the
 * steady-state lookup path performs no allocation.
 */
static inline int
hoplite_host_registry_register(hoplite_host_registry_t *registry,
                               hoplite_host_service_t service,
                               const void *provider)
{
    size_t index;

    if (registry == NULL || service.data == NULL || service.len == 0
        || provider == NULL)
    {
        return HOPLITE_HOST_REGISTRY_INVALID;
    }
    for (index = 0; index < registry->count; index++) {
        if (hoplite_host_service_equal(registry->entries[index].service,
                                       service))
        {
            return HOPLITE_HOST_REGISTRY_DUPLICATE;
        }
    }
    if (registry->count == HOPLITE_HOST_REGISTRY_CAPACITY) {
        return HOPLITE_HOST_REGISTRY_FULL;
    }
    registry->entries[registry->count].service = service;
    registry->entries[registry->count].provider = provider;
    registry->count++;
    return HOPLITE_HOST_REGISTRY_OK;
}

static inline const void *
hoplite_host_registry_find(const hoplite_host_registry_t *registry,
                           hoplite_host_service_t service)
{
    size_t index;

    if (registry == NULL || service.data == NULL || service.len == 0) {
        return NULL;
    }
    for (index = 0; index < registry->count; index++) {
        if (hoplite_host_service_equal(registry->entries[index].service,
                                       service))
        {
            return registry->entries[index].provider;
        }
    }
    return NULL;
}

#endif /* HOPLITE_HOST_REGISTRY_H */
