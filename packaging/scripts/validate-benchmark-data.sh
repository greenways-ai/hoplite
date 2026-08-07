#!/usr/bin/env bash
set -euo pipefail

allow_pending=false
if [[ "${1:-}" == "--allow-pending" ]]; then
  allow_pending=true
  shift
fi

http_report="${1:-website/src/data/http-benchmark.json}"
footprint_report="${2:-website/src/data/stack-footprints.json}"

for report in "$http_report" "$footprint_report"; do
  if [[ ! -f "$report" ]]; then
    echo "Benchmark report does not exist: $report" >&2
    exit 1
  fi
done

python3 - "$allow_pending" "$http_report" "$footprint_report" <<'PY'
from __future__ import annotations

import datetime as dt
import json
import re
import sys
from pathlib import Path
from typing import Any

allow_pending = sys.argv[1] == "true"
http_path = Path(sys.argv[2])
footprint_path = Path(sys.argv[3])
http = json.loads(http_path.read_text())
footprint = json.loads(footprint_path.read_text())


def fail(message: str) -> None:
    raise SystemExit(message)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def positive(value: Any, label: str) -> None:
    require(isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0,
            f"{label} must be a positive number; got {value!r}")


def nonnegative(value: Any, label: str) -> None:
    require(isinstance(value, (int, float)) and not isinstance(value, bool) and value >= 0,
            f"{label} must be a non-negative number; got {value!r}")


def sha(value: Any, label: str) -> None:
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value) is not None,
            f"{label} must be a complete lowercase commit SHA")


def timestamp(value: Any, label: str) -> None:
    require(isinstance(value, str), f"{label} must be an ISO-8601 timestamp")
    try:
        dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        fail(f"{label} is not an ISO-8601 timestamp: {error}")


def contract(value: Any, label: str, *, measured: bool) -> None:
    require(isinstance(value, dict), f"{label} must be an object")
    require(value.get("status") == 200, f"{label}.status must be 200")
    require(value.get("contentType") == "text/plain; charset=utf-8",
            f"{label}.contentType must be text/plain; charset=utf-8")
    require(value.get("xHoplite") == "true", f"{label}.xHoplite must be true")
    require(value.get("bodyBytes") == 19, f"{label}.bodyBytes must be 19")
    body_sha = value.get("bodySha256")
    if measured:
        require(isinstance(body_sha, str) and re.fullmatch(r"[0-9a-f]{64}", body_sha) is not None,
                f"{label}.bodySha256 must be a lowercase SHA-256")
    else:
        require(body_sha is None, f"{label}.bodySha256 must be null while pending")


require(http.get("schemaVersion") == 2, "HTTP report must use schemaVersion 2")
require(footprint.get("schemaVersion") == 2, "Footprint report must use schemaVersion 2")
require(http.get("benchmark") == "equivalent-payload-http", "Unexpected HTTP benchmark identifier")
require(footprint.get("benchmark") == "deployment-footprints", "Unexpected footprint benchmark identifier")
require(http.get("status") == footprint.get("status"), "Benchmark reports must share one status")
status = http.get("status")
require(status in {"pending", "measured"}, f"Unsupported benchmark status: {status!r}")

require(http["targets"]["hoplite"].get("executable") == "/usr/local/bin/hoplite-server",
        "HTTP report must measure /usr/local/bin/hoplite-server")
require(http["targets"]["nginx"].get("executable") == "/opt/nginx/sbin/nginx",
        "HTTP report must measure /opt/nginx/sbin/nginx")
require(footprint["components"]["hoplite"].get("artifact") == "/usr/local/bin/hoplite-server",
        "Footprint report must measure /usr/local/bin/hoplite-server")
require(footprint["components"]["java"].get("label") == "JDK HTTP server sample",
        "Java footprint label must describe the JDK sample")
require(footprint["components"]["python"].get("label") == "Python stdlib HTTP server sample",
        "Python footprint label must describe the stdlib sample")
require(footprint["components"]["lua"].get("label") == "Nginx + ngx_http_lua sample",
        "Lua footprint label must not claim the full OpenResty distribution")

if status == "pending":
    require(allow_pending, "Measured benchmark reports are required; pending placeholders were supplied")
    require(http.get("generatedAt") is None and http.get("commit") is None,
            "Pending HTTP report must not claim generation provenance")
    require(footprint.get("generatedAt") is None and footprint.get("commit") is None,
            "Pending footprint report must not claim generation provenance")
    contract(http.get("responseContract"), "http.responseContract", measured=False)
    contract(footprint.get("responseContract"), "footprint.responseContract", measured=False)
    require(http["responseContract"].get("matchedAcrossTargets") is None,
            "Pending HTTP report must not claim a matched response")
    require(footprint["responseContract"].get("matchedAcrossTargets") is None,
            "Pending footprint report must not claim a matched response")
    for name, target in http["targets"].items():
        require(target.get("samples") == [], f"Pending HTTP target {name} must have no samples")
        for key, value in target.get("metrics", {}).items():
            require(value is None, f"Pending HTTP target {name}.{key} must be null")
        for key in ("imageSizeMiB", "executableSizeMiB", "processCount", "nginxWorkerCount", "idleMemoryMiB"):
            require(target.get(key) is None, f"Pending HTTP target {name}.{key} must be null")
    for name, stack in footprint["stacks"].items():
        for key in ("processCount", "deploymentImageMiB", "idleMemoryMiB", "primaryArtifactMiB"):
            require(stack.get(key) is None, f"Pending footprint stack {name}.{key} must be null")
    print(f"Validated pending benchmark placeholders:\n  {http_path}\n  {footprint_path}")
    raise SystemExit(0)

# Measured report validation.
timestamp(http.get("generatedAt"), "http.generatedAt")
timestamp(footprint.get("generatedAt"), "footprint.generatedAt")
sha(http.get("commit"), "http.commit")
sha(footprint.get("commit"), "footprint.commit")
sha(http.get("provenance", {}).get("haraRevision"), "http.provenance.haraRevision")
sha(footprint.get("provenance", {}).get("haraRevision"), "footprint.provenance.haraRevision")
positive(http.get("logicalCpus"), "http.logicalCpus")
positive(footprint.get("logicalCpus"), "footprint.logicalCpus")
contract(http.get("responseContract"), "http.responseContract", measured=True)
contract(footprint.get("responseContract"), "footprint.responseContract", measured=True)
require(http["responseContract"].get("matchedAcrossTargets") is True,
        "Measured HTTP report must match the response across targets")
require(footprint["responseContract"].get("matchedAcrossTargets") is True,
        "Measured footprint report must match the response across targets")

rounds = http.get("load", {}).get("rounds")
require(isinstance(rounds, int) and rounds >= 3, "HTTP report must contain at least three rounds")
positive(http["load"].get("threads"), "http.load.threads")
positive(http["load"].get("connections"), "http.load.connections")
require(http.get("methodology", {}).get("scheduling") == "Target order alternates on every round",
        "HTTP report must declare alternating target order")

all_sequences: list[int] = []
for name, target in http["targets"].items():
    contract(target.get("responseContract"), f"http.targets.{name}.responseContract", measured=True)
    require(target["responseContract"]["bodySha256"] == http["responseContract"]["bodySha256"],
            f"HTTP target {name} returned a different body")
    for key in ("imageSizeMiB", "executableSizeMiB", "processCount", "nginxWorkerCount", "idleMemoryMiB"):
        positive(target.get(key), f"http.targets.{name}.{key}")
    metrics = target.get("metrics", {})
    positive(metrics.get("requestsPerSecond"), f"http.targets.{name}.metrics.requestsPerSecond")
    nonnegative(metrics.get("latencyP50Ms"), f"http.targets.{name}.metrics.latencyP50Ms")
    nonnegative(metrics.get("latencyP99Ms"), f"http.targets.{name}.metrics.latencyP99Ms")
    positive(metrics.get("peakMemoryMiB"), f"http.targets.{name}.metrics.peakMemoryMiB")
    samples = target.get("samples")
    require(isinstance(samples, list) and len(samples) == rounds,
            f"HTTP target {name} must contain {rounds} samples")
    for sample in samples:
        round_number = sample.get("round")
        require(isinstance(round_number, int) and 1 <= round_number <= rounds,
                f"HTTP target {name} has an invalid round")
        expected_order = 1 if ((round_number % 2 == 1) == (name == "hoplite")) else 2
        require(sample.get("orderInRound") == expected_order,
                f"HTTP target {name} round {round_number} has the wrong alternating order")
        require(isinstance(sample.get("sequence"), int), f"HTTP target {name} sample sequence is invalid")
        all_sequences.append(sample["sequence"])
        positive(sample.get("requestsPerSecond"), f"http.targets.{name}.sample.requestsPerSecond")
        nonnegative(sample.get("latencyP50Ms"), f"http.targets.{name}.sample.latencyP50Ms")
        nonnegative(sample.get("latencyP99Ms"), f"http.targets.{name}.sample.latencyP99Ms")
        positive(sample.get("peakMemoryMiB"), f"http.targets.{name}.sample.peakMemoryMiB")
require(sorted(all_sequences) == list(range(1, rounds * 2 + 1)),
        "HTTP sample sequence must cover every alternating invocation exactly once")
positive(http.get("comparison", {}).get("throughputPercentOfNginx"),
         "http.comparison.throughputPercentOfNginx")

for name, component in footprint["components"].items():
    contract(component.get("responseContract"), f"footprint.components.{name}.responseContract", measured=True)
    require(component["responseContract"]["bodySha256"] == footprint["responseContract"]["bodySha256"],
            f"Footprint component {name} returned a different body")
    for key in ("imageSizeMiB", "artifactSizeMiB", "processCount", "idleMemoryMiB"):
        positive(component.get(key), f"footprint.components.{name}.{key}")
for name, stack in footprint["stacks"].items():
    positive(stack.get("serviceCount"), f"footprint.stacks.{name}.serviceCount")
    for key in ("processCount", "deploymentImageMiB", "idleMemoryMiB", "primaryArtifactMiB"):
        positive(stack.get(key), f"footprint.stacks.{name}.{key}")

require(http["commit"] == footprint["commit"], "Benchmark reports must share the Hoplite commit")
require(http["provenance"]["haraRevision"] == footprint["provenance"]["haraRevision"],
        "Benchmark reports must share the Hara revision")
require(http["provenance"]["nginxVersion"] == footprint["provenance"]["nginxVersion"],
        "Benchmark reports must share the Nginx version")
require(http["responseContract"]["bodySha256"] == footprint["responseContract"]["bodySha256"],
        "Benchmark reports must share the response identity")

print(f"Validated measured benchmark reports:\n  {http_path}\n  {footprint_path}")
PY
