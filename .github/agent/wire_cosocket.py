#!/usr/bin/env python3
from __future__ import annotations

import json
from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}:\n{old}")
    file.write_text(text.replace(old, new))


def insert_if_missing(path: str, marker: str, anchor: str, replacement: str) -> None:
    file = Path(path)
    text = file.read_text()
    if marker in text:
        return
    count = text.count(anchor)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}:\n{anchor}")
    file.write_text(text.replace(anchor, replacement))


module = "core/nginx/ngx_http_hoplite_module.c"
insert_if_missing(
    module,
    '#include "cosocket/hoplite_cosocket.h"',
    '#include "hoplite_runtime.h"\n',
    '#include "hoplite_runtime.h"\n#include "cosocket/hoplite_cosocket.h"\n',
)

cleanup_helper = '''int32_t
hoplite_host_request_cleanup_add_v1(
    void *request_context,
    uint64_t work,
    void *data,
    hoplite_host_request_cleanup_v1_pt cleanup)
{
    ngx_http_hoplite_ctx_t *ctx = request_context;
    ngx_http_cleanup_t *entry;

    if (ctx == NULL || ctx->done || ctx->request == NULL
        || ctx->work != work || work == 0 || cleanup == NULL)
    {
        return HOPLITE_HOST_RESOURCE_ERROR;
    }
    entry = ngx_http_cleanup_add(ctx->request, 0);
    if (entry == NULL) {
        return HOPLITE_HOST_RESOURCE_ERROR;
    }
    entry->handler = cleanup;
    entry->data = data;
    return HOPLITE_HOST_RESOURCE_OK;
}

'''
insert_if_missing(
    module,
    "hoplite_host_request_cleanup_add_v1(",
    "int32_t\nhoplite_host_request_body_read_v1(\n",
    cleanup_helper + "int32_t\nhoplite_host_request_body_read_v1(\n",
)

insert_if_missing(
    module,
    "hoplite_cosocket_register(cycle)",
    '''    if (ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_nginx_provider) != NGX_OK
        || ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_rtc_provider) != NGX_OK)
''',
    '''    if (ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_nginx_provider) != NGX_OK
        || ngx_http_hoplite_provider_register(
            &ngx_http_hoplite_rtc_provider) != NGX_OK
        || hoplite_cosocket_register(cycle) != NGX_OK)
''',
)

insert_if_missing(
    module,
    "hoplite_cosocket_worker_exit();",
    '''    (void) cycle;
    ngx_http_hoplite_rtc_clear();
''',
    '''    (void) cycle;
    hoplite_cosocket_worker_exit();
    ngx_http_hoplite_rtc_clear();
''',
)

cosocket_path = Path("core/nginx/cosocket/hoplite_cosocket.c")
cosocket = cosocket_path.read_text()
writer_old = "hoplite_cosocket_writer_t writer;"
writer_new = "hoplite_cosocket_writer_t writer = {NULL, 0, 0};"
if writer_old in cosocket:
    if cosocket.count(writer_old) < 5:
        raise SystemExit("expected all cosocket result writers before initialization")
    cosocket = cosocket.replace(writer_old, writer_new)

declaration_old = '''    u_char chunk[HOPLITE_COSOCKET_READ_CHUNK];
    size_t amount;
    ssize_t received;
'''
declaration_new = '''    u_char chunk[HOPLITE_COSOCKET_READ_CHUNK];
    size_t amount, line_index, suffix;
    ssize_t received;
'''
if declaration_old in cosocket:
    if cosocket.count(declaration_old) != 1:
        raise SystemExit("expected one receive-drive local declaration")
    cosocket = cosocket.replace(declaration_old, declaration_new)

leftover_old = '''        } else if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_LINE) {
            if (hoplite_cosocket_receive_line_data(socket,
                                                   socket->leftover,
                                                   socket->leftover_len,
                                                   &complete) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            socket->leftover_len = 0;
            if (complete) {
                return hoplite_cosocket_complete_receive(socket, 1, NULL);
            }
'''
leftover_new = '''        } else if (socket->receive_mode == HOPLITE_COSOCKET_RECEIVE_LINE) {
            complete = 0;
            for (line_index = 0;
                 line_index < socket->leftover_len;
                 line_index++)
            {
                if (socket->leftover[line_index] == '\\n') {
                    if (hoplite_cosocket_receive_append(
                            socket, socket->leftover, line_index) != NGX_OK)
                    {
                        hoplite_cosocket_reset_connection(socket);
                        return hoplite_cosocket_complete_receive(
                            socket, 0, "buffer too small");
                    }
                    suffix = socket->leftover_len - line_index - 1;
                    if (suffix != 0) {
                        ngx_memmove(socket->leftover,
                                    socket->leftover + line_index + 1,
                                    suffix);
                    }
                    socket->leftover_len = suffix;
                    if (socket->receive_len != 0
                        && socket->receive_data[socket->receive_len - 1] == '\\r')
                    {
                        socket->receive_len--;
                    }
                    complete = 1;
                    break;
                }
            }
            if (complete) {
                return hoplite_cosocket_complete_receive(socket, 1, NULL);
            }
            if (hoplite_cosocket_receive_append(socket,
                                                 socket->leftover,
                                                 socket->leftover_len) != NGX_OK)
            {
                hoplite_cosocket_reset_connection(socket);
                return hoplite_cosocket_complete_receive(
                    socket, 0, "buffer too small");
            }
            socket->leftover_len = 0;
'''
if leftover_old in cosocket:
    if cosocket.count(leftover_old) != 1:
        raise SystemExit("expected one line-mode leftover block")
    cosocket = cosocket.replace(leftover_old, leftover_new)
cosocket_path.write_text(cosocket)

registry_path = Path("docs/public-surfaces.json")
registry = json.loads(registry_path.read_text())
if not any(entry.get("name") == "hoplite.socket" for entry in registry["hal_namespaces"]):
    socket_entry = {
        "name": "hoplite.socket",
        "path": "core/lib/src/hoplite/socket.hal",
        "status": "experimental",
        "conformance": [
            "core/lib/test/hoplite/socket_test.hal",
            "core/nginx/cosocket/hoplite_cosocket.c",
            "packaging/scripts/smoke-cosocket-tcp.sh",
        ],
        "summary": "OpenResty-compatible request-scoped TCP and UDP cosockets on the Nginx event loop.",
    }
    rtc_index = next(
        index
        for index, entry in enumerate(registry["hal_namespaces"])
        if entry.get("name") == "hoplite.rtc"
    )
    registry["hal_namespaces"].insert(rtc_index + 1, socket_entry)
registry_path.write_text(json.dumps(registry, indent=2) + "\n")

insert_if_missing(
    "website/astro.config.mjs",
    '{ label: "hoplite.socket", slug: "reference/hoplite-socket" }',
    '            { label: "hoplite.rtc", slug: "reference/hoplite-rtc" },\n',
    '            { label: "hoplite.rtc", slug: "reference/hoplite-rtc" },\n'
    '            { label: "hoplite.socket", slug: "reference/hoplite-socket" },\n',
)

insert_if_missing(
    "website/src/content/docs/concepts/host-capabilities.mdx",
    "## Cosockets",
    "## Development host\n",
    '''## Cosockets

`hoplite.socket` exposes OpenResty-compatible TCP and UDP cosocket names through
typed Hara functions. The production implementation is integrated with the
Nginx event loop; a suspended connect, send, or receive does not block the
worker. Socket handles are request-scoped, owner-checked, cancellation-aware,
and closed exactly once.

The first production slice supports numeric-address TCP connect, bounded send,
fixed/line/all receive, close, and independent timeouts. DNS, keepalive pools,
TLS, Unix sockets, delimiter iterators, concurrent directional operations, and
UDP advance under [issue #163](https://github.com/greenways-ai/hoplite/issues/163).
Application code does not name the native `hoplite.socket` service or its
operation strings.

## Development host
''',
)

insert_if_missing(
    "docs/public-api.md",
    "### `hoplite.socket` — experimental",
    "### `hoplite.response-source` — experimental\n",
    '''### `hoplite.socket` — experimental

`hoplite.socket/0-alpha` is the request-scoped OpenResty-compatible cosocket
surface. Namespace functions own the native service and operation identities;
ordinary application code receives opaque typed descriptors and familiar
`[value error]` or `[data error partial]` results.

The first production slice supplies nonblocking numeric-address TCP connect,
bounded send, fixed/line/all receive, explicit close, separate connect/send/read
timeouts, and exactly-once request cleanup. Nginx owns every file descriptor and
readiness event. A socket handle is validated against its request, work, worker,
kind, and native generation and cannot be persisted or transferred.

DNS, keepalive pools, TLS, Unix sockets, delimiter iterators, simultaneous one
reader and one writer, and UDP remain experimental stages of the same namespace.
No incomplete feature is implemented by blocking an Nginx worker.

### `hoplite.response-source` — experimental
''',
)

public_api = Path("docs/public-api.md")
public_api_text = public_api.read_text()
provider_old = '''Provider calls carry request context, work, call, operation, standalone HTA
arguments, and completion callbacks. Cancellation and `release_work` release
retained state without leaking request-body or response-source ownership.
'''
provider_new = '''Provider calls carry request context, work, call, operation, standalone HTA
arguments, and completion callbacks. Providers may register opaque exactly-once
cleanup under the verified request/work scope for idle resources such as
cosockets. Cancellation and `release_work` release retained state without
leaking request-body or response-source ownership.
'''
if "Providers may register opaque exactly-once" not in public_api_text:
    if public_api_text.count(provider_old) != 1:
        raise SystemExit("docs/public-api.md: provider paragraph anchor changed")
    public_api.write_text(public_api_text.replace(provider_old, provider_new))

ci_path = Path(".github/workflows/ci.yml")
ci = ci_path.read_text()
ci_marker = "Build the TCP cosocket production fixture"
ci_anchor = '''      - name: Prove aliases, referred Vars and ordered namespaces through Nginx
        run: bash hoplite/packaging/scripts/smoke-multi-module.sh hoplite-multi-module
'''
ci_replacement = '''      - name: Prove aliases, referred Vars and ordered namespaces through Nginx
        run: bash hoplite/packaging/scripts/smoke-multi-module.sh hoplite-multi-module
      - name: Build the TCP cosocket production fixture
        run: |
          docker build \\
            -f hoplite/packaging/docker/Dockerfile \\
            --build-arg HOPLITE_APP=packaging/fixtures/cosocket-tcp \\
            -t hoplite-cosocket-tcp \\
            .
      - name: Verify the cosocket fixture remains source-free
        run: bash hoplite/packaging/scripts/assert-production-image.sh hoplite-cosocket-tcp
      - name: Prove TCP connect, send, receive and cleanup on the Nginx event loop
        run: bash hoplite/packaging/scripts/smoke-cosocket-tcp.sh hoplite-cosocket-tcp
'''
if ci_marker not in ci:
    if ci.count(ci_anchor) != 1:
        raise SystemExit(".github/workflows/ci.yml: multi-module smoke anchor changed")
    ci_path.write_text(ci.replace(ci_anchor, ci_replacement))
