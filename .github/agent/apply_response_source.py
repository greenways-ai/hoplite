#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / "core/nginx/ngx_http_hoplite_module.c"
BLOCK = Path(__file__).with_name("nginx_response_source_block.c")
SEND_RESULT = Path(__file__).with_name("nginx_send_result.c")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one anchor, found {count}")
    return text.replace(old, new, 1)


def main() -> None:
    text = MODULE.read_text()
    if "NGX_HTTP_HOPLITE_RESPONSE_SOURCE_CHUNK" in text:
        raise SystemExit("response-source integration already applied")

    text = replace_once(
        text,
        '#include "hoplite_blob_host_provider.h"\n#include "hoplite_hta.h"',
        '#include "hoplite_blob_host_provider.h"\n#include "hoplite_hta.h"\n#include "hoplite_response_source.h"',
        "response-source include",
    )
    text = replace_once(
        text,
        "#define NGX_HTTP_HOPLITE_RAW 0\n"
        "#define NGX_HTTP_HOPLITE_REQUEST 1\n"
        "#define NGX_HTTP_HOPLITE_REQUEST_HTA 2",
        "#define NGX_HTTP_HOPLITE_RAW 0\n"
        "#define NGX_HTTP_HOPLITE_REQUEST 1\n"
        "#define NGX_HTTP_HOPLITE_REQUEST_HTA 2\n"
        "#define NGX_HTTP_HOPLITE_RESPONSE_SOURCE_CHUNK (64u * 1024u)",
        "response-source chunk bound",
    )
    body_type = """typedef struct {
    ngx_http_request_t *request;
    ngx_chain_t *chain;
    off_t offset;
    ngx_flag_t closed;
} ngx_http_hoplite_body_t;
"""
    stream_type = body_type + """
typedef struct {
    hoplite_response_source_state_v1_t source;
    ngx_buf_t *buffer;
    ngx_chain_t chain;
    u_char *storage;
    size_t capacity;
    ngx_flag_t active;
    ngx_flag_t submitted;
    ngx_flag_t last;
} ngx_http_hoplite_response_source_t;
"""
    text = replace_once(
        text, body_type, stream_type, "response-source request state"
    )
    text = replace_once(
        text,
        "    ngx_http_hoplite_body_t body;\n};",
        "    ngx_http_hoplite_body_t body;\n"
        "    ngx_http_hoplite_response_source_t response_source;\n"
        "};",
        "response-source context field",
    )
    text = replace_once(
        text,
        "static ngx_int_t ngx_http_hoplite_drain(ngx_log_t *log);",
        "static ngx_int_t ngx_http_hoplite_drain(ngx_log_t *log);\n"
        "static void ngx_http_hoplite_response_source_write_handler(\n"
        "    ngx_http_request_t *request);\n"
        "static void ngx_http_hoplite_response_source_release(\n"
        "    ngx_http_hoplite_ctx_t *ctx);",
        "response-source forward declarations",
    )
    text = replace_once(
        text,
        "static void\n"
        "ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)\n"
        "{\n"
        "    if (ctx->done) {\n"
        "        return;\n"
        "    }\n"
        "    ctx->done = 1;",
        "static void\n"
        "ngx_http_hoplite_finish(ngx_http_hoplite_ctx_t *ctx)\n"
        "{\n"
        "    if (ctx->done) {\n"
        "        return;\n"
        "    }\n"
        "    ngx_http_hoplite_response_source_release(ctx);\n"
        "    ctx->done = 1;",
        "response-source finish ordering",
    )

    block = BLOCK.read_text().rstrip()
    text = replace_once(
        text,
        "\nstatic ngx_int_t\nngx_http_hoplite_send_native(",
        "\n" + block + "\n\nstatic ngx_int_t\nngx_http_hoplite_send_native(",
        "response-source Nginx writer",
    )

    start = text.find("static void\nngx_http_hoplite_send_result(")
    end = text.find("\nstatic void\nngx_http_hoplite_send_error(", start)
    if start < 0 or end < 0:
        raise SystemExit("response result function anchors were not found")
    replacement = SEND_RESULT.read_text().rstrip()
    text = text[:start] + replacement + "\n" + text[end:]

    text = replace_once(
        text,
        "static void\n"
        "ngx_http_hoplite_cleanup(void *data)\n"
        "{\n"
        "    ngx_http_hoplite_ctx_t *ctx = data;\n\n"
        "    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {",
        "static void\n"
        "ngx_http_hoplite_cleanup(void *data)\n"
        "{\n"
        "    ngx_http_hoplite_ctx_t *ctx = data;\n\n"
        "    ngx_http_hoplite_response_source_release(ctx);\n"
        "    if (ctx->provider != NULL && ctx->provider->cancel != NULL) {",
        "response-source cleanup ordering",
    )

    MODULE.write_text(text)


if __name__ == "__main__":
    main()
