import httpSource from "../data/http-benchmark.json";
import footprintSource from "../data/stack-footprints.json";

export type Measurement = number | null;

export interface ResponseContract {
  status: number;
  contentType: string;
  xHoplite: string;
  bodyBytes: number;
  bodySha256: string | null;
  matchedAcrossTargets?: boolean | null;
}

export interface BenchmarkProvenance {
  haraRevision: string | null;
  nginxVersion: string | null;
  workflowRunId: number | null;
  workflowRunUrl: string | null;
}

export interface HttpSample {
  round: number;
  orderInRound: number;
  sequence: number;
  requestsPerSecond: number;
  latencyP50Ms: number;
  latencyP99Ms: number;
  peakMemoryMiB: number;
}

export interface HttpTarget {
  id: string;
  label: string;
  image: string | null;
  imageId: string | null;
  executable: string;
  imageSizeMiB: Measurement;
  executableSizeMiB: Measurement;
  processCount: Measurement;
  nginxWorkerCount: Measurement;
  idleMemoryMiB: Measurement;
  responseContract: ResponseContract | null;
  metrics: {
    requestsPerSecond: Measurement;
    latencyP50Ms: Measurement;
    latencyP99Ms: Measurement;
    peakMemoryMiB: Measurement;
  };
  samples: HttpSample[];
}

export interface HttpBenchmarkReport {
  schemaVersion: number;
  status: "pending" | "measured";
  benchmark: "equivalent-payload-http";
  generatedAt: string | null;
  commit: string | null;
  runner: string | null;
  cpu: string | null;
  logicalCpus: Measurement;
  provenance: BenchmarkProvenance;
  load: {
    threads: number;
    connections: number;
    duration: string;
    rounds: number;
  };
  methodology: {
    warmup: string;
    scheduling: string;
    idleSamplesPerTarget: number;
    memoryAccounting: string;
    scope: string;
  };
  responseContract: ResponseContract;
  payload: {
    bodyBytes: number;
    sha256: string | null;
    body: string;
  };
  targets: {
    hoplite: HttpTarget;
    nginx: HttpTarget;
  };
  comparison: {
    throughputPercentOfNginx: Measurement;
    requestRateDeltaPercent: Measurement;
    p50DeltaMs: Measurement;
    p99DeltaMs: Measurement;
    idleMemoryDeltaMiB: Measurement;
    peakMemoryDeltaMiB: Measurement;
    imageSizeDeltaMiB: Measurement;
    executableSizeDeltaMiB: Measurement;
  };
}

export interface FootprintComponent {
  id: string;
  label: string;
  kind: string;
  image: string | null;
  imageId: string | null;
  artifact: string;
  responseContract: ResponseContract | null;
  imageSizeMiB: Measurement;
  artifactSizeMiB: Measurement;
  processCount: Measurement;
  nginxWorkerCount: Measurement;
  idleMemoryMiB: Measurement;
}

export interface FootprintStack {
  label: string;
  components: string[];
  serviceCount: number;
  processCount: Measurement;
  deploymentImageMiB: Measurement;
  idleMemoryMiB: Measurement;
  primaryArtifactMiB: Measurement;
}

export interface FootprintReport {
  schemaVersion: number;
  status: "pending" | "measured";
  benchmark: "deployment-footprints";
  generatedAt: string | null;
  commit: string | null;
  runner: string | null;
  cpu: string | null;
  logicalCpus: Measurement;
  provenance: BenchmarkProvenance;
  responseContract: ResponseContract;
  methodology: {
    idleSamplesPerComponent: number;
    payloadBytes: number;
    imageAccounting: string;
    memoryAccounting: string;
    scope: string;
  };
  components: {
    hoplite: FootprintComponent;
    nginx: FootprintComponent;
    java: FootprintComponent;
    python: FootprintComponent;
    lua: FootprintComponent;
  };
  stacks: {
    hoplite: FootprintStack;
    java: FootprintStack;
    python: FootprintStack;
    lua: FootprintStack;
  };
}

export const httpBenchmark = httpSource as unknown as HttpBenchmarkReport;
export const stackFootprints = footprintSource as unknown as FootprintReport;

export const hasMeasuredHttpBenchmark = (report = httpBenchmark) =>
  report.schemaVersion === 2 &&
  report.status === "measured" &&
  report.responseContract.matchedAcrossTargets === true;

export const hasMeasuredFootprints = (report = stackFootprints) =>
  report.schemaVersion === 2 &&
  report.status === "measured" &&
  report.responseContract.matchedAcrossTargets === true;
