#!/usr/bin/env node

import fs from "node:fs";

const FORMAT = "hoplite.runtime-measurement/0-alpha";
const COMPARISON = "hoplite.runtime-comparison/0-alpha";
const sampleGroups = {
  startupNs: ["configuration", "bundle", "modules", "routes", "total"],
  sizesBytes: ["server", "image", "bundle", "manifest", "configuration"],
  memoryBytes: ["worker1", "worker4", "marginalWorker"],
  requests: ["syncLatencyNs", "suspendedLatencyNs", "syncAllocations", "streamingPeakBytes"],
};
const environmentFields = ["os", "kernel", "architecture", "cpu", "logicalCpus", "totalMemoryBytes"];

function fail(message) {
  throw new Error(message);
}

function object(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(`${name} must be an object`);
  return value;
}

function text(value, name) {
  if (typeof value !== "string" || value.length === 0) fail(`${name} must be non-empty text`);
}

function validate(report) {
  object(report, "report");
  if (report.format !== FORMAT) fail(`format must be ${FORMAT}`);
  for (const name of ["hopliteRevision", "haraRevision"]) {
    if (!/^[0-9a-f]{40}$/.test(report[name])) fail(`${name} must be a complete lowercase commit SHA`);
  }
  if (typeof report.dirty !== "boolean") fail("dirty must be boolean");
  text(report.fixture, "fixture");
  text(report.requestIdentity, "requestIdentity");
  if (!Number.isInteger(report.warmups) || report.warmups < 0) fail("warmups must be a non-negative integer");
  if (!Array.isArray(report.workerCounts) || report.workerCounts.length === 0 || report.workerCounts.some(value => !Number.isInteger(value) || value < 1)) fail("workerCounts must contain positive integers");
  const environment = object(report.environment, "environment");
  for (const name of environmentFields) {
    const value = environment[name];
    if (name === "logicalCpus" || name === "totalMemoryBytes") {
      if (!Number.isFinite(value) || value <= 0) fail(`environment.${name} must be positive`);
    } else text(value, `environment.${name}`);
  }
  const tools = object(report.tools, "tools");
  for (const name of ["rustc", "nginx", "docker"]) text(tools[name], `tools.${name}`);
  const samples = object(report.samples, "samples");
  for (const [group, names] of Object.entries(sampleGroups)) {
    const values = object(samples[group], `samples.${group}`);
    for (const name of names) {
      if (!Array.isArray(values[name]) || values[name].length === 0 || values[name].some(value => !Number.isFinite(value) || value < 0)) fail(`samples.${group}.${name} must contain non-negative raw numbers`);
    }
  }
  return report;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function compare(baseline, candidate) {
  validate(baseline);
  validate(candidate);
  const incompatibilities = environmentFields.filter(name => baseline.environment[name] !== candidate.environment[name]);
  const deltas = {};
  for (const [group, names] of Object.entries(sampleGroups)) {
    deltas[group] = {};
    for (const name of names) {
      const before = median(baseline.samples[group][name]);
      const after = median(candidate.samples[group][name]);
      deltas[group][name] = { baselineMedian: before, candidateMedian: after, delta: after - before };
    }
  }
  return { format: COMPARISON, compatibleEnvironment: incompatibilities.length === 0, incompatibilities, deltas };
}

function fixture(delta = 0) {
  const samples = {};
  for (const [group, names] of Object.entries(sampleGroups)) {
    samples[group] = Object.fromEntries(names.map(name => [name, [1 + delta, 2 + delta, 3 + delta]]));
  }
  return {
    format: FORMAT,
    hopliteRevision: "1".repeat(40),
    haraRevision: "2".repeat(40),
    dirty: false,
    fixture: "generic-multi-module",
    requestIdentity: "GET /hello; sha256:example",
    workerCounts: [1, 4],
    warmups: 3,
    environment: { os: "linux", kernel: "test", architecture: "x86_64", cpu: "test-cpu", logicalCpus: 4, totalMemoryBytes: 1024 },
    tools: { rustc: "rustc test", nginx: "nginx test", docker: "docker test" },
    samples,
  };
}

const args = process.argv.slice(2);
try {
  if (args[0] === "--self-test") {
    const result = compare(fixture(), fixture(1));
    if (!result.compatibleEnvironment || result.deltas.startupNs.total.delta !== 1) fail("comparison self-test failed");
    const incompatible = fixture(1);
    incompatible.environment.cpu = "different";
    if (compare(fixture(), incompatible).compatibleEnvironment) fail("environment self-test failed");
    console.log("runtime measurement contract self-test passed");
  } else if (args.length === 1) {
    validate(JSON.parse(fs.readFileSync(args[0], "utf8")));
    console.log(`validated ${FORMAT}`);
  } else if (args.length === 2) {
    console.log(JSON.stringify(compare(
      JSON.parse(fs.readFileSync(args[0], "utf8")),
      JSON.parse(fs.readFileSync(args[1], "utf8")),
    ), null, 2));
  } else {
    fail("usage: validate-runtime-measurement.mjs REPORT [CANDIDATE] | --self-test");
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
