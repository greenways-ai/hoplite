import { access, readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const websiteRoot = fileURLToPath(new URL("../", import.meta.url));
const distRoot = join(websiteRoot, "dist");
const requireMeasured = process.env.HOPLITE_REQUIRE_MEASURED_BENCHMARKS === "1";
const legacyRuntimeModelPath = "hopliteconcepts/runtime-model/index.html";
const expectedRuntimeModelUrl = "/hoplite/concepts/runtime-model/";
const legacyHostMarker = "data-gw-legacy-host-redirect";
const required = [
  "index.html",
  "getting-started/installation/index.html",
  "concepts/data-plane-providers/index.html",
  "guides/writing-web-services/index.html",
  "guides/provider-distributions/index.html",
  "reference/data-plane-protocols/index.html",
  "reference/hoplite-value/index.html",
  "reference/hoplite-response-source/index.html",
  "reference/hoplite-auth/index.html",
  "benchmarks/http.json",
  "benchmarks/footprints.json",
  legacyRuntimeModelPath,
];
const expectedHomepageCopy = [
  "Your application, directly inside Nginx.",
  "Choose where it runs.",
  "Hoplite against raw Nginx.",
  "What each stack occupies.",
  "See what happens—and why.",
  "Why it matters",
];
const expectedLaunchMarkers = [
  "data-launch-console",
  'data-launch-target="docker"',
  'data-launch-target="homebrew"',
  'data-launch-target="linux"',
  'data-launch-target="fly"',
];
const expectedNavigationLinks = ["/hoplite/", "/hoplite/getting-started/"];
const expectedArchitectureCopy = [
  "Application authentication",
  "Data-plane providers",
  "Provider distributions",
  "Native provider protocols",
  "hoplite.response-source/1",
  "hoplite.value-request/1",
  "hoplite-blob-provider-v0.1.1",
];
const retiredArchitectureClaims = [
  "Hoplite authenticates both management users and application callers.",
  "Foreground operation also starts the Hoplite management gateway",
  "safe platform defaults: user-owned keys",
  "The request body is not yet included in the HTA request map.",
  "Authentication is owned by Hoplite, not by an application module.",
  "Initialize and inspect Hoplite-owned authentication",
];
const expectedProjectLinks = [
  "https://oss.greenways.ai/",
  "https://oss.greenways.ai/hestia/",
  "https://oss.greenways.ai/hoplite/",
  "https://oss.greenways.ai/historia/",
  "https://oss.greenways.ai/hodos/",
];
const retiredMeasurements = [
  "44.272736",
  "44.3 MiB",
  "60.690000",
  "74.2 MiB",
  "ae9946502661e8146e2d1a97ad8dedff35ca285d",
];
const unscopedRootLink = /(href|src|srcset|action)="\/(?!hoplite(?:\/|"))/;
const duplicatedScope = /(href|src|srcset|action)="\/hoplite\/hoplite(?:\/|[A-Za-z0-9_-])/;
const referenceContracts = {
  "hoplite-core": ["example.app", "Failure behavior", "package-ref", ":hoplite/type :response"],
  "hoplite-dev": ["example.app", "Common failures", ":status :running", "16384"],
  "hoplite-host": ["example.crypto", "Signature verification", "00ff10", "4096"],
  "hoplite-internal": ["example.host", "example.admin", ":profile/main", "configuration errors"],
  "hoplite-response-source": ["example.download", "Using a provider-owned body", "Validation and failures", "69632"],
  "hoplite-value": ["example.values", "Request and result flow", "Validation failures", "object-missing"],
};

async function htmlFiles(directory) {
  const output = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) output.push(...(await htmlFiles(path)));
    if (entry.isFile() && entry.name.endsWith(".html")) output.push(path);
  }
  return output;
}

function pageUrl(path) {
  const documentPath = relative(distRoot, path).replaceAll("\\", "/");
  const pathname = documentPath === "index.html"
    ? "/hoplite/"
    : `/hoplite/${documentPath.replace(/index\.html$/, "")}`;
  return new URL(pathname, "https://oss.greenways.ai");
}

async function localTarget(pathname) {
  const scoped = decodeURIComponent(pathname).replace(/^\/hoplite\/?/, "");
  const candidates = scoped === ""
    ? [join(distRoot, "index.html")]
    : pathname.endsWith("/")
      ? [join(distRoot, scoped, "index.html"), join(distRoot, `${scoped.replace(/\/$/, "")}.html`)]
      : [join(distRoot, scoped), join(distRoot, scoped, "index.html"), join(distRoot, `${scoped}.html`)];
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next static-output shape.
    }
  }
  return null;
}

const positive = (value, label) => {
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${label} is not a positive measurement`);
};
const nonnegative = (value, label) => {
  if (!Number.isFinite(value) || value < 0) throw new Error(`${label} is not a non-negative measurement`);
};
const responseContractValid = (contract) =>
  contract?.status === 200 &&
  contract?.contentType === "text/plain; charset=utf-8" &&
  contract?.xHoplite === "true" &&
  contract?.bodyBytes === 19;

for (const path of required) await access(join(distRoot, path));
const pages = await htmlFiles(distRoot);
if (pages.length === 0) throw new Error("Pages verification found no generated HTML files");
let renderedDocumentation = "";

for (const path of pages) {
  const source = await readFile(path, "utf8");
  renderedDocumentation += source;
  if (!source.includes(legacyHostMarker)) {
    throw new Error(`Pages verification did not find the legacy-host redirect in ${path}`);
  }
  const unscoped = source.match(unscopedRootLink);
  if (unscoped) throw new Error(`Unscoped root link in ${path}: ${unscoped[0]}`);
  const duplicated = source.match(duplicatedScope);
  if (duplicated) throw new Error(`Duplicated base path in ${path}: ${duplicated[0]}`);

  for (const href of [...source.matchAll(/href="([^"]+)"/g)].map((match) => match[1])) {
    if (/^(?:mailto:|tel:|javascript:)/.test(href)) continue;
    const url = new URL(href, pageUrl(path));
    if (url.origin !== "https://oss.greenways.ai" || !url.pathname.startsWith("/hoplite")) continue;
    if (!(await localTarget(url.pathname))) throw new Error(`Broken internal link in ${path}: ${href}`);
  }
}

for (const copy of expectedArchitectureCopy) {
  if (!renderedDocumentation.includes(copy)) {
    throw new Error(`Missing architecture documentation contract: ${copy}`);
  }
}
for (const retired of retiredArchitectureClaims) {
  if (renderedDocumentation.includes(retired)) {
    throw new Error(`Retired architecture claim remains published: ${retired}`);
  }
}

for (const [slug, markers] of Object.entries(referenceContracts)) {
  const reference = await readFile(join(distRoot, "reference", slug, "index.html"), "utf8");
  for (const marker of markers) {
    if (!reference.includes(marker)) {
      throw new Error(`${slug} is missing richer reference content: ${marker}`);
    }
  }
}

const referenceShell = await readFile(join(distRoot, "reference/hoplite-core/index.html"), "utf8");
for (const shellMarker of ["data-has-sidebar", "sidebar-pane", "<details", "<summary", 'aria-controls="starlight__sidebar"']) {
  if (!referenceShell.includes(shellMarker)) {
    throw new Error(`Reference pages no longer expose expected collapsible navigation: ${shellMarker}`);
  }
}
if (referenceShell.includes('href="/hoplite/reference/hoplite-auth/"')) {
  throw new Error("Legacy hoplite.auth remains in the reference navigation");
}

const legacyAuth = await readFile(join(distRoot, "reference/hoplite-auth/index.html"), "utf8");
for (const marker of ["Removed from current releases", "Do not add", "/hoplite/guides/authentication/"]) {
  if (!legacyAuth.includes(marker)) throw new Error(`Legacy auth migration page is missing: ${marker}`);
}

const home = await readFile(join(distRoot, "index.html"), "utf8");
for (const href of [...expectedNavigationLinks, ...expectedProjectLinks]) {
  if (!home.includes(`href="${href}"`)) throw new Error(`Missing navigation link: ${href}`);
}
for (const marker of [
  "data-gw-documentation-header",
  "data-gw-documentation-search",
  "data-gw-project-switcher",
  "data-gw-theme-button",
]) {
  if (!home.includes(marker)) throw new Error(`Missing shared documentation control: ${marker}`);
}
for (const copy of expectedHomepageCopy) {
  if (!home.includes(copy)) throw new Error(`Missing homepage contract: ${copy}`);
}
for (const marker of expectedLaunchMarkers) {
  if (!home.includes(marker)) throw new Error(`Missing launch surface marker: ${marker}`);
}
if (!home.includes('href="https://github.com/greenways-ai/hoplite"')) {
  throw new Error("Homepage source link is missing");
}

const httpSourcePath = join(websiteRoot, "src/data/http-benchmark.json");
const footprintSourcePath = join(websiteRoot, "src/data/stack-footprints.json");
const httpReport = JSON.parse(await readFile(httpSourcePath, "utf8"));
const footprintReport = JSON.parse(await readFile(footprintSourcePath, "utf8"));
if (httpReport.schemaVersion !== 2 || footprintReport.schemaVersion !== 2) {
  throw new Error("Pages requires schema-v2 benchmark reports");
}
if (httpReport.status !== footprintReport.status) {
  throw new Error("HTTP and footprint reports do not share one publication state");
}
if (!responseContractValid(httpReport.responseContract) || !responseContractValid(footprintReport.responseContract)) {
  throw new Error("Benchmark reports do not declare the stable Hoplite response contract");
}
if (httpReport.targets.hoplite.executable !== "/usr/local/bin/hoplite-server") {
  throw new Error("HTTP report does not describe hoplite-server");
}
if (footprintReport.components.hoplite.artifact !== "/usr/local/bin/hoplite-server") {
  throw new Error("Footprint report does not describe hoplite-server");
}

if (httpReport.status === "pending") {
  if (requireMeasured) throw new Error("Measured benchmark reports were required but pending placeholders were supplied");
  for (const target of Object.values(httpReport.targets)) {
    if (target.samples.length !== 0) throw new Error("Pending HTTP targets must not contain measured rounds");
    for (const value of Object.values(target.metrics)) {
      if (value !== null) throw new Error("Pending HTTP targets must not contain metric values");
    }
    for (const key of ["imageSizeMiB", "executableSizeMiB", "idleMemoryMiB", "processCount", "nginxWorkerCount"]) {
      if (target[key] !== null) throw new Error(`Pending HTTP target contains ${key}`);
    }
  }
  for (const stale of retiredMeasurements) {
    if (home.includes(stale)) throw new Error(`Pending site rendered retired measurement: ${stale}`);
  }
  if (!home.includes("Fresh post-split measurement pending")) {
    throw new Error("Pending site does not explain that fresh post-split measurements are pending");
  }
} else if (httpReport.status === "measured") {
  if (httpReport.responseContract.matchedAcrossTargets !== true) {
    throw new Error("Measured HTTP response contract was not matched across targets");
  }
  if (!httpReport.payload.sha256 || httpReport.payload.sha256.length !== 64) {
    throw new Error("Measured HTTP payload has no SHA-256 identity");
  }
  for (const [targetName, target] of Object.entries(httpReport.targets)) {
    if (!Array.isArray(target.samples) || target.samples.length !== httpReport.load.rounds || httpReport.load.rounds < 3) {
      throw new Error(`${targetName} does not contain the declared measured rounds`);
    }
    positive(target.metrics.requestsPerSecond, `${targetName}.requestsPerSecond`);
    nonnegative(target.metrics.latencyP50Ms, `${targetName}.latencyP50Ms`);
    nonnegative(target.metrics.latencyP99Ms, `${targetName}.latencyP99Ms`);
    positive(target.metrics.peakMemoryMiB, `${targetName}.peakMemoryMiB`);
    for (const key of ["imageSizeMiB", "executableSizeMiB", "idleMemoryMiB", "processCount", "nginxWorkerCount"]) {
      positive(target[key], `${targetName}.${key}`);
    }
  }
  positive(httpReport.comparison.throughputPercentOfNginx, "comparison.throughputPercentOfNginx");
  if (!home.includes("All measured rounds")) throw new Error("Measured site does not expose its measured rounds");
} else {
  throw new Error(`Unsupported HTTP benchmark status: ${httpReport.status}`);
}

if (footprintReport.status === "pending") {
  for (const stack of Object.values(footprintReport.stacks)) {
    for (const key of ["deploymentImageMiB", "idleMemoryMiB", "primaryArtifactMiB", "processCount"]) {
      if (stack[key] !== null) throw new Error(`Pending footprint stack contains ${key}`);
    }
  }
} else {
  if (footprintReport.responseContract.matchedAcrossTargets !== true) {
    throw new Error("Measured footprint response contract was not matched across targets");
  }
  for (const [stackName, stack] of Object.entries(footprintReport.stacks)) {
    for (const key of ["deploymentImageMiB", "idleMemoryMiB", "primaryArtifactMiB", "processCount"]) {
      positive(stack[key], `${stackName}.${key}`);
    }
  }
}

for (const label of [
  "Requests / second",
  "p50 latency",
  "p99 latency",
  "Peak memory under load",
  "Runtime executable",
  "Deployment image",
  "Idle memory",
  "Plain Nginx",
  "Deploy image",
  "Measured shape",
]) {
  if (!home.includes(label)) throw new Error(`Benchmark label is missing: ${label}`);
}

const builtHttp = JSON.parse(await readFile(join(distRoot, "benchmarks/http.json"), "utf8"));
const builtFootprints = JSON.parse(await readFile(join(distRoot, "benchmarks/footprints.json"), "utf8"));
if (JSON.stringify(builtHttp) !== JSON.stringify(httpReport)) {
  throw new Error("Published HTTP JSON does not match the website source report");
}
if (JSON.stringify(builtFootprints) !== JSON.stringify(footprintReport)) {
  throw new Error("Published footprint JSON does not match the website source report");
}

const legacyRuntimeModel = await readFile(join(distRoot, legacyRuntimeModelPath), "utf8");
if (
  !legacyRuntimeModel.includes(`content="0; url=${expectedRuntimeModelUrl}"`) ||
  !legacyRuntimeModel.includes(`href="${expectedRuntimeModelUrl}"`)
) {
  throw new Error(`Legacy runtime-model redirect does not point to ${expectedRuntimeModelUrl}`);
}

console.log(
  `Verified ${pages.length} Pages documents with ${httpReport.status} schema-v2 benchmark reports, scoped links, raw report endpoints, and canonical redirects.`,
);
