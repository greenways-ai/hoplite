from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(relative: str) -> str:
    return (ROOT / relative).read_text(encoding="utf-8")


def write(relative: str, content: str) -> None:
    path = ROOT / relative
    path.write_text(content, encoding="utf-8")


def replace_once(relative: str, before: str, after: str) -> None:
    content = read(relative)
    count = content.count(before)
    if count != 1:
        raise RuntimeError(f"{relative}: expected one exact replacement, found {count}")
    write(relative, content.replace(before, after, 1))


def sub_once(relative: str, pattern: str, replacement: str) -> None:
    content = read(relative)
    updated, count = re.subn(pattern, replacement, content, count=1, flags=re.DOTALL)
    if count != 1:
        raise RuntimeError(f"{relative}: expected one regex replacement, found {count}")
    write(relative, updated)


RAW_SOURCE = r'''(ns hoplite.raw
  (:config {}))

(defn method [exchange]
  (:method exchange))

(defn uri [exchange]
  (:uri exchange))

(defn path [exchange]
  (:path exchange))

(defn query-string [exchange]
  (:query-string exchange))

(defn remote-address [exchange]
  (:remote-address exchange))

(defn body-handle [exchange]
  (:body-handle exchange))

(defn identity [exchange]
  (:identity exchange))

(defn header [exchange name]
  (get (:headers exchange) name))

(defn headers [exchange]
  (:headers exchange))

(defn sleep
  "Returns a Promise completed by the Nginx worker timer."
  [milliseconds]
  (Host/call "nginx" "raw/sleep" [milliseconds]))

(defn variable
  "Returns one allowlisted request-scoped Nginx variable as text or nil."
  [name]
  (Host/call "nginx" "raw/variable" [name]))

(defn scheme []
  (variable :scheme))

(defn protocol []
  (variable :protocol))

(defn host []
  (variable :host))

(defn server-name []
  (variable :server-name))

(defn server-address []
  (variable :server-address))

(defn server-port []
  (variable :server-port))

(defn remote-port []
  (variable :remote-port))

(defn request-id []
  (variable :request-id))

(defn connection-id []
  (variable :connection-id))

(defn connection-requests []
  (variable :connection-requests))

(defn request-time []
  (variable :request-time))

(defn log!
  "Writes one bounded request-scoped message to the Nginx error log."
  [level message]
  (Host/call "nginx" "raw/log" [level message]))

(defn respond!
  ([exchange status body]
   (hoplite.raw.native/respond exchange status {} body))
  ([exchange status headers body]
   (hoplite.raw.native/respond exchange status headers body)))

(defn start! [exchange status headers]
  (hoplite.raw.native/start exchange status headers))

(defn write! [exchange chunk]
  (hoplite.raw.native/write exchange chunk))

(defn finish! [exchange]
  (hoplite.raw.native/finish exchange))
'''
write("core/lib/src/hoplite/raw.hal", RAW_SOURCE)

C_BLOCK = r'''static ngx_flag_t
ngx_http_hoplite_operation(const ngx_str_t *actual, const char *expected)
{
    size_t len = ngx_strlen(expected);
    return actual->len == len
        && ngx_strncmp(actual->data, expected, len) == 0;
}

typedef struct {
    ngx_str_t public_name;
    ngx_str_t nginx_name;
} ngx_http_hoplite_raw_variable_t;

static ngx_http_hoplite_raw_variable_t ngx_http_hoplite_raw_variables[] = {
    { ngx_string("scheme"), ngx_string("scheme") },
    { ngx_string("protocol"), ngx_string("server_protocol") },
    { ngx_string("host"), ngx_string("host") },
    { ngx_string("server-name"), ngx_string("server_name") },
    { ngx_string("server-address"), ngx_string("server_addr") },
    { ngx_string("server-port"), ngx_string("server_port") },
    { ngx_string("remote-port"), ngx_string("remote_port") },
    { ngx_string("request-id"), ngx_string("request_id") },
    { ngx_string("connection-id"), ngx_string("connection") },
    { ngx_string("connection-requests"), ngx_string("connection_requests") },
    { ngx_string("request-time"), ngx_string("request_time") }
};

static ngx_int_t
ngx_http_hoplite_nginx_resolve_nil(
    const ngx_http_hoplite_host_call_t *call)
{
    return hoplite_call_resolve(ngx_http_hoplite_runtime,
                                call->call, NULL, 0) == 0
        ? NGX_OK : NGX_ERROR;
}

static ngx_int_t
ngx_http_hoplite_nginx_sleep(const ngx_http_hoplite_host_call_t *call)
{
    int64_t delay;
    ngx_http_hoplite_sleep_t *sleep;

    if (call->arguments == NULL
        || call->arguments->kind != HOPLITE_HTA_VECTOR
        || call->arguments->as.vector.count != 1
        || hoplite_hta_number(call->arguments->as.vector.items[0], &delay)
               != NGX_OK
        || delay < 0 || delay > 3600000)
    {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.raw/sleep expects milliseconds from 0 to 3600000");
    }
    if (delay == 0) {
        return ngx_http_hoplite_nginx_resolve_nil(call);
    }
    if (call->ctx->sleep != NULL) {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "request already has a pending Hoplite operation");
    }

    sleep = ngx_pcalloc(call->ctx->request->pool, sizeof(*sleep));
    if (sleep == NULL) {
        return NGX_ERROR;
    }
    sleep->ctx = call->ctx;
    sleep->call = call->call;
    sleep->event.handler = ngx_http_hoplite_sleep_handler;
    sleep->event.data = sleep;
    sleep->event.log = call->log;
    call->ctx->sleep = sleep;
    ngx_add_timer(&sleep->event, (ngx_msec_t) delay);
    return NGX_AGAIN;
}

static ngx_int_t
ngx_http_hoplite_nginx_variable(const ngx_http_hoplite_host_call_t *call)
{
    ngx_http_hoplite_raw_variable_t *variable = NULL;
    ngx_http_variable_value_t *value;
    ngx_str_t requested, encoded, text;
    ngx_uint_t i;

    if (call->arguments == NULL
        || call->arguments->kind != HOPLITE_HTA_VECTOR
        || call->arguments->as.vector.count != 1
        || hoplite_hta_text(call->arguments->as.vector.items[0], &requested)
               != NGX_OK)
    {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.raw/variable expects one allowlisted variable name");
    }

    for (i = 0;
         i < sizeof(ngx_http_hoplite_raw_variables)
                 / sizeof(ngx_http_hoplite_raw_variables[0]);
         i++)
    {
        if (requested.len == ngx_http_hoplite_raw_variables[i].public_name.len
            && ngx_strncmp(requested.data,
                           ngx_http_hoplite_raw_variables[i].public_name.data,
                           requested.len) == 0)
        {
            variable = &ngx_http_hoplite_raw_variables[i];
            break;
        }
    }
    if (variable == NULL) {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.raw/variable is not allowlisted");
    }

    value = ngx_http_get_variable(
        call->ctx->request,
        &variable->nginx_name,
        ngx_hash_key(variable->nginx_name.data, variable->nginx_name.len));
    if (value == NULL) {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.raw/variable lookup failed");
    }
    if (value->not_found) {
        return ngx_http_hoplite_nginx_resolve_nil(call);
    }

    text.data = value->data;
    text.len = value->len;
    if (hoplite_hta_encode_string(call->pool, &text, &encoded) != NGX_OK) {
        return NGX_ERROR;
    }
    return hoplite_call_resolve(ngx_http_hoplite_runtime,
                                call->call, encoded.data, encoded.len) == 0
        ? NGX_OK : NGX_ERROR;
}

static ngx_int_t
ngx_http_hoplite_nginx_log(const ngx_http_hoplite_host_call_t *call)
{
    ngx_str_t level_name, message;
    ngx_uint_t i, level;

    if (call->arguments == NULL
        || call->arguments->kind != HOPLITE_HTA_VECTOR
        || call->arguments->as.vector.count != 2
        || hoplite_hta_text(call->arguments->as.vector.items[0], &level_name)
               != NGX_OK
        || hoplite_hta_text(call->arguments->as.vector.items[1], &message)
               != NGX_OK
        || message.len > 4096)
    {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.raw/log! expects a level and at most 4096 bytes of text");
    }

    for (i = 0; i < message.len; i++) {
        if (message.data[i] == '\0'
            || message.data[i] == '\r'
            || message.data[i] == '\n')
        {
            return ngx_http_hoplite_reject(
                call->call, call->pool,
                "hoplite.raw/log! message contains a line break");
        }
    }

    if (ngx_http_hoplite_operation(&level_name, "debug")) {
        level = NGX_LOG_DEBUG;
    } else if (ngx_http_hoplite_operation(&level_name, "info")) {
        level = NGX_LOG_INFO;
    } else if (ngx_http_hoplite_operation(&level_name, "notice")) {
        level = NGX_LOG_NOTICE;
    } else if (ngx_http_hoplite_operation(&level_name, "warn")) {
        level = NGX_LOG_WARN;
    } else if (ngx_http_hoplite_operation(&level_name, "error")) {
        level = NGX_LOG_ERR;
    } else {
        return ngx_http_hoplite_reject(
            call->call, call->pool,
            "hoplite.raw/log! level must be debug, info, notice, warn, or error");
    }

    ngx_log_error(level, call->log, 0, "hoplite.raw: %V", &message);
    return ngx_http_hoplite_nginx_resolve_nil(call);
}

static ngx_int_t
ngx_http_hoplite_nginx_invoke(const ngx_http_hoplite_host_call_t *call)
{
    if (ngx_http_hoplite_operation(&call->operation, "sleep")
        || ngx_http_hoplite_operation(&call->operation, "raw/sleep"))
    {
        return ngx_http_hoplite_nginx_sleep(call);
    }
    if (ngx_http_hoplite_operation(&call->operation, "raw/variable")) {
        return ngx_http_hoplite_nginx_variable(call);
    }
    if (ngx_http_hoplite_operation(&call->operation, "raw/log")) {
        return ngx_http_hoplite_nginx_log(call);
    }
    return ngx_http_hoplite_reject(call->call, call->pool,
                                   "unsupported nginx host operation");
}

static ngx_http_hoplite_rtc_session_t *'''

sub_once(
    "core/nginx/ngx_http_hoplite_module.c",
    r"static ngx_int_t\nngx_http_hoplite_nginx_invoke\(.*?\nstatic ngx_http_hoplite_rtc_session_t \*",
    C_BLOCK,
)

replace_once(
    "core/examples/app.hal",
    '(ns hoplite.app\n  (:require [hoplite.core :as h]))',
    '(ns hoplite.app\n  (:require [hoplite.core :as h]\n            [hoplite.raw :as raw]))',
)
replace_once(
    "core/examples/app.hal",
    '(std.native.Host/call "nginx" "sleep" [25])',
    '(raw/sleep 25)',
)
replace_once(
    "core/examples/app.hal",
    '(std.native.Host/call "nginx" "sleep" [-1])',
    '(raw/sleep -1)',
)
replace_once(
    "core/examples/app.hal",
    '(std.native.Host/call "nginx" "sleep" [1]))\n  {:status 200\n   :body {:protocol "hoplite.response-source/0-alpha"',
    '(raw/sleep 1))\n  {:status 200\n   :body {:protocol "hoplite.response-source/0-alpha"',
)
replace_once(
    "core/examples/app.hal",
    '(std.native.Host/call "nginx" "sleep" [1]))\n  {:status 200\n   :body (response-source-descriptor',
    '(raw/sleep 1))\n  {:status 200\n   :body (response-source-descriptor',
)

RAW_HANDLERS = r'''
(defn ^:async raw-request-context
  [exchange]
  (let [scheme (std.foundation.coroutine/await (raw/scheme))
        protocol (std.foundation.coroutine/await (raw/protocol))
        host (std.foundation.coroutine/await (raw/host))
        server-name (std.foundation.coroutine/await (raw/server-name))
        server-address (std.foundation.coroutine/await (raw/server-address))
        server-port (std.foundation.coroutine/await (raw/server-port))
        remote-port (std.foundation.coroutine/await (raw/remote-port))
        request-id (std.foundation.coroutine/await (raw/request-id))
        connection-id (std.foundation.coroutine/await (raw/connection-id))
        connection-requests (std.foundation.coroutine/await
                             (raw/connection-requests))
        request-time (std.foundation.coroutine/await (raw/request-time))]
    (std.foundation.coroutine/await
     (raw/log! :warn "request context exposed through hoplite.raw"))
    {:status 200
     :headers {"content-type" "text/plain; charset=utf-8"
               "x-hoplite-raw-method" (raw/method exchange)
               "x-hoplite-raw-path" (raw/path exchange)
               "x-hoplite-raw-scheme" scheme
               "x-hoplite-raw-protocol" protocol
               "x-hoplite-raw-host" host
               "x-hoplite-raw-server-name" server-name
               "x-hoplite-raw-server-address" server-address
               "x-hoplite-raw-server-port" server-port
               "x-hoplite-raw-remote-port" remote-port
               "x-hoplite-raw-connection-id" connection-id
               "x-hoplite-raw-connection-requests" connection-requests
               "x-hoplite-raw-request-time" request-time}
     :body request-id}))

(defn ^:async forbidden-raw-variable
  [_exchange]
  (std.foundation.coroutine/await
   (raw/variable :document-root))
  {:status 200 :body "unexpected raw variable success\n"})
'''
replace_once(
    "core/examples/app.hal",
    '\n(def app\n  (h/app',
    RAW_HANDLERS + '\n(def app\n  (h/app',
)

RAW_ROUTES = r'''      ["/raw/context"
       {:get {:name "raw-request-context"
              :summary "Reads the closed Nginx request context through hoplite.raw"
              :route/adapter :raw
              :handler #'raw-request-context}}]
      ["/raw/forbidden-variable"
       {:get {:name "raw-forbidden-variable"
              :summary "Rejects Nginx variables outside the public allowlist"
              :route/adapter :raw
              :handler #'forbidden-raw-variable}}]
'''
replace_once(
    "core/examples/app.hal",
    '      ["/body-handle"\n',
    RAW_ROUTES + '      ["/body-handle"\n',
)

RUNTIME_HOST_TEST = r'''    #[test]
    fn hoplite_raw_names_nginx_host_operations() {
        let mut runtime = HopliteRuntime::new();
        let source = format!(
            "{}\n(ns raw.operations) \\
             (defn ^:async inspect [] \\
               (let [scheme (Coroutine/await (hoplite.raw/scheme))] \\
                 (Coroutine/await (hoplite.raw/log! :info scheme)) \\
                 scheme)) \\
             (inspect)",
            include_str!("../../lib/src/hoplite/raw.hal"),
        );
        let work = runtime.work_start(&source, None);

        let Value::Vector(variable_call) = take_event(&mut runtime) else {
            panic!("raw variable host event")
        };
        assert!(matches!(variable_call.get(0), Some(Value::Number(2))));
        assert!(matches!(variable_call.get(5), Some(Value::String(service)) if service == "nginx"));
        assert!(matches!(variable_call.get(6), Some(Value::String(method)) if method == "raw/variable"));
        let variable_call_id = match variable_call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("raw variable call id"),
        };
        let variable_arguments = match variable_call.get(7) {
            Some(Value::Vector(arguments)) => arguments,
            _ => panic!("raw variable arguments"),
        };
        assert!(matches!(variable_arguments.peek_first(), Some(Value::Keyword(name)) if name.as_str() == "scheme"));

        runtime
            .call_deliver(variable_call_id, true, Value::String("https".into()))
            .unwrap();
        let Value::Vector(log_call) = take_event(&mut runtime) else {
            panic!("raw log host event")
        };
        assert!(matches!(log_call.get(0), Some(Value::Number(2))));
        assert!(matches!(log_call.get(5), Some(Value::String(service)) if service == "nginx"));
        assert!(matches!(log_call.get(6), Some(Value::String(method)) if method == "raw/log"));
        let log_call_id = match log_call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("raw log call id"),
        };
        let log_arguments = match log_call.get(7) {
            Some(Value::Vector(arguments)) => arguments,
            _ => panic!("raw log arguments"),
        };
        assert!(matches!(log_arguments.get(0), Some(Value::Keyword(level)) if level.as_str() == "info"));
        assert!(matches!(log_arguments.get(1), Some(Value::String(message)) if message == "https"));

        runtime.call_deliver(log_call_id, true, Value::Nil).unwrap();
        let Value::Vector(done) = take_event(&mut runtime) else {
            panic!("raw operation completion event")
        };
        assert!(matches!(done.get(0), Some(Value::Number(0))));
        assert!(matches!(done.get(1), Some(Value::Number(value)) if *value == work as i64));
        assert!(matches!(done.get(2), Some(Value::String(value)) if value == "https"));
    }

'''
replace_once(
    "core/runtime/src/lib.rs",
    '    #[test]\n    fn trusted_hoplite_host_intrinsics_complete_synchronously() {',
    RUNTIME_HOST_TEST
    + '    #[test]\n    fn trusted_hoplite_host_intrinsics_complete_synchronously() {',
)

RAW_RUNTIME_TEST = r'''    #[test]
    fn hoplite_raw_adapter_uses_the_response_api() {
        let mut runtime = HopliteRuntime::new();
        let source = format!(
            "{}\n(ns direct.raw) \\
             (defn show [exchange] \\
               (hoplite.raw/respond! exchange 201 \\
                 {\"x-mode\" \"raw\"} \\
                 (hoplite.raw/path exchange))) \\
             nil",
            include_str!("../../lib/src/hoplite/raw.hal"),
        );
        runtime.work_start(&source, None);
        let _ = take_event(&mut runtime);
        let works_before = runtime.works.len();
        runtime
            .apps_prepare(manifest_v2("direct.raw/show", "raw"))
            .unwrap();
        let mut context = TestRequest { headers: vec![] };
        let outcome = runtime
            .app_invoke(1, test_request(&mut context, "/raw"))
            .unwrap();
        let InvokeState::Complete(response) = outcome else {
            panic!("raw route suspended")
        };
        let response = runtime.responses.get(&response).unwrap();
        assert_eq!(response.status, 201);
        assert!(matches!(&response.body, NativeResponseBody::Buffered(body) if body == b"/raw"));
        assert_eq!(response.headers, vec![("x-mode".into(), "raw".into())]);
        assert_eq!(runtime.works.len(), works_before);
    }

    #[test]
    fn request_v3_projects_only_an_opaque_body_handle'''
sub_once(
    "core/runtime/src/lib.rs",
    r"    #\[test\]\n    fn raw_adapter_uses_the_exchange_response_api\(\) \{.*?\n    \}\n\n    #\[test\]\n    fn request_v3_projects_only_an_opaque_body_handle",
    RAW_RUNTIME_TEST,
)

SMOKE_RAW = r'''
status="$(request "$base/raw/context")"
if [[ "$status" != 200 ]] \
  || ! grep -Eiq '^x-hoplite-raw-method: GET\r?$' "$headers_file" \
  || ! grep -Eiq '^x-hoplite-raw-path: /raw/context\r?$' "$headers_file" \
  || ! grep -Eiq '^x-hoplite-raw-scheme: http\r?$' "$headers_file" \
  || ! grep -Eiq '^x-hoplite-raw-protocol: HTTP/1\.[01]\r?$' "$headers_file" \
  || ! grep -Eiq '^x-hoplite-raw-host: 127\.0\.0\.1\r?$' "$headers_file" \
  || ! grep -Eiq '^x-hoplite-raw-server-port: [0-9]+\r?$' "$headers_file" \
  || ! grep -Eiq '^x-hoplite-raw-remote-port: [0-9]+\r?$' "$headers_file" \
  || ! grep -Eq '^[0-9a-fA-F]{32}$' "$body_file"; then
  echo "hoplite.raw request context failed: status=$status body=$(cat "$body_file")" >&2
  cat "$headers_file" >&2
  diagnose
  exit 1
fi

if ! docker logs "$container" 2>&1 \
    | grep -Fq 'hoplite.raw: request context exposed through hoplite.raw'; then
  echo 'hoplite.raw request-scoped logging was not observed.' >&2
  diagnose
  exit 1
fi

status="$(request "$base/raw/forbidden-variable")"
if [[ "$status" != 500 ]]; then
  echo "hoplite.raw accepted a variable outside its allowlist: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi
'''
replace_once(
    "packaging/scripts/smoke-host-providers.sh",
    '\nfor route in unknown-service unknown-operation invalid-arguments; do\n',
    SMOKE_RAW + '\nfor route in unknown-service unknown-operation invalid-arguments; do\n',
)

DOC_BEFORE = '''### `hoplite.raw` — public

Accessors expose method, URI, path, query string, remote address, and headers
from the borrowed exchange. `respond!`, `start!`, `write!`, and `finish!`
operate on that exchange.

The exchange is valid only for the active request invocation. Applications must
not retain it after completion, cancellation, disconnect, or worker shutdown.
The native host owns the exchange and backing memory.
'''
DOC_AFTER = '''### `hoplite.raw` — public

`hoplite.raw` is the named application boundary for Nginx-backed request work.
Applications use this namespace rather than constructing `"nginx"`
`Host/call` messages themselves.

Synchronous accessors expose method, URI, path, query string, remote address,
headers, the optional opaque body handle, and the optional authenticated
identity from the borrowed exchange. `respond!`, `start!`, `write!`, and
`finish!` construct responses against that exchange.

Promise-returning operations compose with Hara coroutines while the Nginx event
loop retains request ownership. `sleep` uses a worker timer. `variable` and its
named helpers expose only this closed request-variable set:

- `scheme`, `protocol`, `host`, `server-name`, `server-address`, `server-port`;
- `remote-port`, `request-id`, `connection-id`, `connection-requests`; and
- `request-time`.

`log!` writes one request-scoped message of at most 4096 bytes at `:debug`,
`:info`, `:notice`, `:warn`, or `:error`. The namespace intentionally does not
expose arbitrary Nginx variables or directives, filesystem roots, upstream
selection, native pointers, or unrestricted subrequests.

The exchange and every operation are valid only for the active request
invocation. Applications must not retain them after completion, cancellation,
disconnect, or worker shutdown. The native host owns the exchange and backing
memory.
'''
replace_once("docs/public-api.md", DOC_BEFORE, DOC_AFTER)
replace_once(
    "docs/public-api.md",
    '`hoplite.host` is experimental convenience over generic host calls. Application\nsemantics must remain provider-replaceable.',
    '`hoplite.host` is experimental convenience over generic provider calls.\nApplication semantics must remain provider-replaceable; Nginx-backed request\nfeatures belong in `hoplite.raw`, not application-authored `"nginx"` calls.',
)

replace_once(
    "docs/public-surfaces.json",
    '    {"name":"hoplite.raw","path":"core/lib/src/hoplite/raw.hal","status":"public","conformance":["core/lib/test/hoplite/core_test.hal"],"summary":"Raw request, route-adapter and response-map construction."},',
    '    {"name":"hoplite.raw","path":"core/lib/src/hoplite/raw.hal","status":"public","conformance":["core/runtime/src/lib.rs","packaging/scripts/smoke-host-providers.sh"],"summary":"Borrowed raw exchange, closed Nginx request variables, timers, logging, and response construction."},',
)
