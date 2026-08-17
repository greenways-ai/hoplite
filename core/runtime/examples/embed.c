#include "hoplite_runtime.h"

#include <assert.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

static hoplite_slice_t text(const char *value) {
  hoplite_slice_t slice = {
      .data = (const uint8_t *)value,
      .len = strlen(value),
  };
  return slice;
}

static int32_t raw_field(void *context, uint32_t field,
                         hoplite_slice_t *value) {
  if (context == NULL || value == NULL) {
    return HOPLITE_RAW_FIELD_ERROR;
  }
  value->data = NULL;
  value->len = 0;
  if (field != HOPLITE_RAW_FIELD_SCHEME) {
    return HOPLITE_RAW_FIELD_UNAVAILABLE;
  }
  *value = text((const char *)context);
  return HOPLITE_RAW_FIELD_OK;
}

int main(void) {
  static const char source[] =
      "(ns embedding.example (:require [std.foundation :refer :all])) "
      "(defn respond [exchange] "
      "{:status 200 :headers {\"content-type\" \"text/plain\"} "
      ":body (or (:scheme exchange) \"missing\")})";
  static const char function[] = "embedding.example/respond";
  static const char scheme[] = "https";

  assert(hoplite_abi_version() >= 5);

  hoplite_runtime_t *runtime = hoplite_runtime_new();
  assert(runtime != NULL);
  assert(hoplite_bootstrap_modules(runtime, (const uint8_t *)source,
                                   strlen(source)) == 0);

  uint64_t handler = hoplite_handler_prepare(
      runtime, (const uint8_t *)function, strlen(function));
  assert(handler != 0);

  hoplite_raw_request_v1_t raw = {
      .context = (void *)scheme,
      .field = raw_field,
  };
  hoplite_request_v4_t request = {
      .request =
          {
              .request =
                  {
                      .context = NULL,
                      .method = text("GET"),
                      .uri = text("/embed"),
                      .path = text("/embed"),
                      .query_string = text(""),
                      .remote_address = text("127.0.0.1"),
                      .header_count = 0,
                      .header_at = NULL,
                  },
              .body = NULL,
              .max_body_bytes = 0,
              .max_chunk_bytes = 0,
              .require_declared_length = 0,
          },
      .raw = &raw,
  };
  hoplite_outcome_v2_t outcome = {0};
  assert(hoplite_handler_invoke_v4(runtime, handler, 0, &request,
                                   &outcome) == 0);
  assert(outcome.kind == 1);
  assert(outcome.id != 0);

  uint16_t status = 0;
  assert(hoplite_response_status_v2(runtime, outcome.id, &status) == 0);
  assert(status == 200);

  hoplite_slice_t body = {0};
  assert(hoplite_response_body_v2(runtime, outcome.id, &body) == 0);
  assert(body.data != NULL);
  assert(body.len == strlen(scheme));
  assert(memcmp(body.data, scheme, body.len) == 0);

  assert(hoplite_response_close_v2(runtime, outcome.id) == 0);
  assert(hoplite_handler_close(runtime, handler) == 0);
  hoplite_runtime_free(runtime);
  return 0;
}
