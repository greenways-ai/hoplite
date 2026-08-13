# Request failure classes

`hoplite.request-failure/0-alpha` gives request-time failures the same redaction
law as startup. Nginx writes a compact document with exactly `format` and
`class`; request values and implementation details are not included.

The current stable classes are:

- `routing` — the selected app or handler cannot be invoked;
- `application` — a handler rejected without exposing its private message;
- `body-limit` — declared request content exceeds the configured bound;
- `host-suspension` — a suspended host operation cannot continue;
- `unsupported-yield` — the runtime returned an unsupported outcome;
- `timeout` — `hoplite_request_timeout` elapsed (30 seconds by default);
- `cancellation` — runtime work was explicitly cancelled;
- `disconnect` — request cleanup ran before completion;
- `response-stream` — ranged or streaming response delivery failed; and
- `cleanup` — exactly-once resource cleanup failed closed.

Timeout returns HTTP 504 and closes the owning work. Body limits return 413.
Disconnect and cancellation close pending provider operations before the work
scope. A streaming failure returns 500 if headers are still mutable; otherwise
the connection is terminated. None of these classes exposes a pointer, route,
path, credential, signature, source form, provider operation, or response body.
