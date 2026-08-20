import fs from "node:fs";

const docsRoot = new URL("../../docs/", import.meta.url);
const matrixUrl = new URL("openresty-cosocket-compatibility.json", docsRoot);
const matrix = JSON.parse(fs.readFileSync(matrixUrl, "utf8"));

const allowedStatuses = new Set([
  "compatible",
  "adapted",
  "restricted",
  "extension",
  "not-planned",
  "pending",
]);

const requiredFields = [
  "id",
  "namespace",
  "function",
  "openresty",
  "hoplite",
  "status",
  "argumentResultMapping",
  "allowedPhases",
  "yielding",
  "timeouts",
  "pooling",
  "cleanupOwner",
  "authority",
  "implementationIssue",
  "evidence",
  "notes",
];

const requiredRows = new Set([
  "descriptor.authority",
  "tcp.constructor",
  "tcp.connect.numeric",
  "tcp.connect.hostname",
  "tcp.connect.unix",
  "tcp.connect.pool-options",
  "tcp.connect.backlog",
  "tcp.send",
  "tcp.receive",
  "tcp.receiveany",
  "tcp.receiveuntil",
  "tcp.shutdown",
  "tcp.settimeout",
  "tcp.settimeouts",
  "tcp.setoption",
  "tcp.setkeepalive",
  "tcp.getreusedtimes",
  "tcp.close",
  "tcp.concurrent-read-write",
  "tcp.sslhandshake",
  "tcp.setclientcert",
  "udp.constructor",
  "udp.setpeername",
  "udp.send",
  "udp.receive",
  "udp.timeouts",
  "udp.close",
]);

function fail(message) {
  throw new Error(`cosocket compatibility matrix: ${message}`);
}

if (matrix.schema !== "hoplite.openresty.cosocket-compatibility/1") {
  fail(`unexpected schema ${JSON.stringify(matrix.schema)}`);
}

if (!/^\d{4}-\d{2}-\d{2}$/.test(matrix.updated ?? "")) {
  fail("updated must be an ISO calendar date");
}

if (!Array.isArray(matrix.operationFiles) || matrix.operationFiles.length === 0) {
  fail("operationFiles must be a non-empty array");
}

for (const status of allowedStatuses) {
  if (typeof matrix.legend?.[status] !== "string" || matrix.legend[status].length === 0) {
    fail(`legend is missing ${status}`);
  }
}

const operations = [];
const seenFiles = new Set();
for (const path of matrix.operationFiles) {
  if (
    typeof path !== "string" ||
    !/^openresty-cosocket-compatibility\/[a-z0-9-]+\.json$/.test(path) ||
    seenFiles.has(path)
  ) {
    fail(`unsafe or duplicate operation file ${JSON.stringify(path)}`);
  }
  seenFiles.add(path);

  const rows = JSON.parse(fs.readFileSync(new URL(path, docsRoot), "utf8"));
  if (!Array.isArray(rows) || rows.length === 0) {
    fail(`${path} must contain a non-empty row array`);
  }
  operations.push(...rows);
}

const seen = new Set();
for (const [index, operation] of operations.entries()) {
  for (const field of requiredFields) {
    if (!(field in operation)) {
      fail(`row ${index} is missing ${field}`);
    }
  }

  if (typeof operation.id !== "string" || operation.id.length === 0) {
    fail(`row ${index} has an invalid id`);
  }
  if (seen.has(operation.id)) {
    fail(`duplicate row id ${operation.id}`);
  }
  seen.add(operation.id);

  if (operation.namespace !== "hoplite.socket") {
    fail(`${operation.id} has unexpected namespace ${operation.namespace}`);
  }
  if (!allowedStatuses.has(operation.status)) {
    fail(`${operation.id} has unknown status ${operation.status}`);
  }
  if (
    typeof operation.implementationIssue !== "string" ||
    !/^https:\/\/github\.com\/greenways-ai\/hoplite\/issues\/\d+$/.test(
      operation.implementationIssue,
    )
  ) {
    fail(`${operation.id} must link one Hoplite implementation issue`);
  }
  if (!Array.isArray(operation.evidence)) {
    fail(`${operation.id} evidence must be an array`);
  }
  if (operation.status !== "pending" && operation.evidence.length === 0) {
    fail(`${operation.id} is delivered but has no evidence`);
  }
  for (const path of operation.evidence) {
    if (
      typeof path !== "string" ||
      path.length === 0 ||
      path.startsWith("/") ||
      path.includes("..")
    ) {
      fail(`${operation.id} contains an unsafe evidence path`);
    }
  }

  for (const field of requiredFields.filter(
    (name) => !["evidence", "notes"].includes(name),
  )) {
    if (typeof operation[field] !== "string" || operation[field].length === 0) {
      fail(`${operation.id} has an empty ${field}`);
    }
  }
}

for (const id of requiredRows) {
  if (!seen.has(id)) {
    fail(`required row ${id} is missing`);
  }
}

const statusCounts = Object.fromEntries(
  [...allowedStatuses].map((status) => [
    status,
    operations.filter((operation) => operation.status === status).length,
  ]),
);

console.log(
  `verified ${operations.length} cosocket compatibility rows in ` +
    `${matrix.operationFiles.length} files: ` +
    Object.entries(statusCounts)
      .map(([status, count]) => `${status}=${count}`)
      .join(", "),
);
