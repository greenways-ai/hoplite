# Startup diagnostics

`hoplite.startup-diagnostic/0-alpha` is the ordered, path-free production
startup report. A worker emits one compact JSON document per completed or failed
stage:

| Sequence | Stage | Owner |
| ---: | --- | --- |
| 1 | `configuration` | Nginx configuration validation |
| 2 | `bundle` | bounded HAB0 and exact-manifest validation |
| 3 | `modules` | transactional HBX0 module loading |
| 4 | `routes` | handler and route preparation |
| 5 | `readiness` | worker publication |

Successful documents contain `format`, `sequence`, `stage`, and `status: "ok"`.
A failure contains `status: "failed"` and one stable `class`. No later stage is
emitted. The class never includes filesystem paths, source, configuration text,
credentials, signatures, native pointers, or extension details.

Runtime ABI 4 exposes `hoplite_bootstrap_application_v2` and
`hoplite_bootstrap_application_files_v2`. Their callback borrows one UTF-8 JSON
document for the callback duration. The older `_v1` symbols remain supported
without reporting.

A checksum failure therefore stops with a single runtime document:

```json
{"format":"hoplite.startup-diagnostic/0-alpha","sequence":2,"stage":"bundle","status":"failed","class":"application-bundle-checksum-mismatch"}
```
