#include "hoplite_host_registry.h"

#include <assert.h>
#include <stdint.h>

static hoplite_host_service_t
service(const char *value, size_t len)
{
    hoplite_host_service_t result;
    result.data = (const uint8_t *) value;
    result.len = len;
    return result;
}

int
main(void)
{
    hoplite_host_registry_t registry;
    static const int nginx_provider = 1;
    static const int tahto_provider = 2;
    static const int other_providers[HOPLITE_HOST_REGISTRY_CAPACITY] = {0};
    static const char *other_services[HOPLITE_HOST_REGISTRY_CAPACITY] = {
        "one", "two", "three", "four", "five", "six", "seven", "eight"
    };
    hoplite_host_service_t nginx = service("nginx", 5);
    size_t index;

    hoplite_host_registry_init(&registry);
    assert(registry.count == 0);
    assert(hoplite_host_registry_register(NULL, nginx, &nginx_provider)
           == HOPLITE_HOST_REGISTRY_INVALID);
    assert(hoplite_host_registry_register(&registry, service(NULL, 0),
                                          &nginx_provider)
           == HOPLITE_HOST_REGISTRY_INVALID);
    assert(hoplite_host_registry_register(&registry, nginx, NULL)
           == HOPLITE_HOST_REGISTRY_INVALID);

    assert(hoplite_host_registry_register(&registry, nginx, &nginx_provider)
           == HOPLITE_HOST_REGISTRY_OK);
    assert(hoplite_host_registry_find(&registry, nginx) == &nginx_provider);
    assert(hoplite_host_registry_find(&registry, service("NGINX", 5)) == NULL);
    assert(hoplite_host_registry_find(&registry, service("ngin", 4)) == NULL);
    assert(hoplite_host_registry_register(&registry, nginx, &tahto_provider)
           == HOPLITE_HOST_REGISTRY_DUPLICATE);

    hoplite_host_registry_init(&registry);
    for (index = 0; index < HOPLITE_HOST_REGISTRY_CAPACITY; index++) {
        assert(hoplite_host_registry_register(
                   &registry,
                   service(other_services[index],
                           strlen(other_services[index])),
                   &other_providers[index])
               == HOPLITE_HOST_REGISTRY_OK);
    }
    assert(registry.count == HOPLITE_HOST_REGISTRY_CAPACITY);
    assert(hoplite_host_registry_register(&registry, nginx, &nginx_provider)
           == HOPLITE_HOST_REGISTRY_FULL);

    return 0;
}
