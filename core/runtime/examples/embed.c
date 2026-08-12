#include "hoplite_runtime.h"

#include <assert.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>

static hoplite_slice_t text(const char *value) {
  hoplite_slice_t slice = {
      .data = (const uint8_t *)value,
      .len = strlen(value),
  };
  return slice;
}

int main(void) {
  static const char source[] =
      "(ns embedding.example (:require [std.foundation :refer :all])) "
      "(defn respond [request] "
      "{:status 200 :headers {\"content-type\" \"text/plain\"} "
      ":body \"embedded\"})";
  static const char function[] = "embedding.example/respond";

  assert(hoplite_abi_version() >= 4);

  hoplite_runtime_t *runtime = hoplite_runtime_new();
  assert(runtime != NULL);
  assert(hoplite_bootstrap_modules(runtime, (const uint8_t *)source,
                                   strlen(source)) == 0);

  uint64_t handler = hoplite_handler_prepare(
      runtime, (const uint8_t *)function, strlen(function));
  assert(handler != 0);

  hoplite_request_v2_t request = {
      .context = NULL,
      .method = text("GET"),
      .uri = text("/embed"),
      .path = text("/embed"),
      .query_string = text(""),
      .remote_address = text("127.0.0.1"),
      .header_count = 0,
      .header_at = NULL,
  };
  hoplite_outcome_v2_t outcome = {0};
  assert(hoplite_handler_invoke_v2(runtime, handler, 1, &request,
                                   &outcome) == 0);
  assert(outcome.kind == 1);
  assert(outcome.id != 0);

  uint16_t status = 0;
  assert(hoplite_response_status_v2(runtime, outcome.id, &status) == 0);
  assert(status == 200);

  hoplite_slice_t body = {0};
  assert(hoplite_response_body_v2(runtime, outcome.id, &body) == 0);
  assert(body.data != NULL);
  assert(body.len == strlen("embedded"));
  assert(memcmp(body.data, "embedded", body.len) == 0);

  assert(hoplite_response_close_v2(runtime, outcome.id) == 0);
  assert(hoplite_handler_close(runtime, handler) == 0);
  hoplite_runtime_free(runtime);
  return 0;
}
