import { access, readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
const root = join(repositoryRoot, "dist");
const legacyRuntimeModelPath = "hopliteconcepts/runtime-model/index.html";
const installationPath = "getting-started/installation/index.html";
const legacyHostMarker = "data-gw-legacy-host-redirect";
const expectedRuntimeModelUrl = "/hoplite/concepts/runtime-model/";
const required = [
  "index.html",
  installationPath,
  "guides/writing-web-services/index.html",
  legacyRuntimeModelPath,
];
const artworkBase = "https://oss.greenways.ai/visual-language/artwork/hoplite/";
const expectedAccentArtwork = ["rabbit-courtyard", "branching-paths"].flatMap((scene) => [
  `${artworkBase}${scene}-day.webp`,
  `${artworkBase}${scene}-night.webp`,
  `${artworkBase}${scene}-day-mobile.webp`,
  `${artworkBase}${scene}-night-mobile.webp`,
]);
const expectedHomepageCopy = [
  "Your application, directly inside Nginx.",
  "Choose where it runs.",
  "Speed you can inspect.",
  "Same host. Less stack to operate.",
  "See what happens—and why.",
  "Why it matters",
];
const expectedLaunchMarkers = [
  "data-launch-console",
  'data-launch-target="docker"',
  'data-launch-target="homebrew"',
  'data-launch-target="linux"',
  'data-launch-target="fly"',
  "ghcr.io/greenways-ai/hoplite:latest",
  "brew install greenways-ai/tap/hoplite",
  "scripts/install.sh",
  "scripts/new-app.sh",
];
const forbiddenHomepageCopy = [
  "Four expressions of air",
  "Configure once. Run anywhere.",
  "git clone https://github.com/greenways-ai/hoplite.git",
  "proxy hops",
  "compile per worker",
  "mandatory request copies",
  "async records on sync routes",
];
const expectedGuideUrl = "https://oss.greenways.ai/hoplite/guides/writing-web-services";
const expectedNavigationLinks = ["/hoplite/", "/hoplite/getting-started/"];
const expectedProjectLinks = [
  "https://greenways.ai/",
  "https://oss.greenways.ai/hestia/",
  "https://oss.greenways.ai/hoplite/",
  "https://oss.greenways.ai/historia/",
  "https://oss.greenways.ai/hodos/",
  "https://statstrade.io/",
];
const unscopedRootLink = /(href|src|srcset|action)="\/(?!hoplite(?:\/|"))/;
const duplicatedScope = /(href|src|srcset|action)="\/hoplite\/hoplite(?:\/|[A-Za-z0-9_-])/;

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
  const documentPath = relative(root, path).replaceAll("\\", "/");
  const pathname = documentPath === "index.html"
    ? "/hoplite/"
    : `/hoplite/${documentPath.replace(/index\.html$/, "")}`;
  return new URL(pathname, "https://oss.greenways.ai");
}

async function localTarget(pathname) {
  const scoped = decodeURIComponent(pathname).replace(/^\/hoplite\/?/, "");
  const candidates = scoped === ""
    ? [join(root, "index.html")]
    : pathname.endsWith("/")
      ? [join(root, scoped, "index.html"), join(root, `${scoped.replace(/\/$/, "")}.html`)]
      : [join(root, scoped), join(root, scoped, "index.html"), join(root, `${scoped}.html`)];

  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next supported static-output shape.
    }
  }
  return null;
}

for (const path of required) await access(join(root, path));

const files = await htmlFiles(root);
if (files.length === 0) throw new Error("Pages verification found no generated HTML files");

for (const path of files) {
  const source = await readFile(path, "utf8");
  if (!source.includes(legacyHostMarker)) {
    throw new Error(`Pages verification did not find the legacy-host redirect in ${path}`);
  }
  const unscoped = source.match(unscopedRootLink);
  if (unscoped) throw new Error(`Pages verification found an unscoped root link in ${path}: ${unscoped[0]}`);
  const duplicated = source.match(duplicatedScope);
  if (duplicated) throw new Error(`Pages verification found a duplicated base path in ${path}: ${duplicated[0]}`);

  const links = [...source.matchAll(/href="([^"]+)"/g)].map((match) => match[1]);
  for (const href of links) {
    if (/^(?:mailto:|tel:|javascript:)/.test(href)) continue;
    const url = new URL(href, pageUrl(path));
    if (url.origin !== "https://oss.greenways.ai") continue;
    if (!url.pathname.startsWith("/hoplite")) continue;
    if (!(await localTarget(url.pathname))) {
      throw new Error(`Pages verification found a broken internal link in ${path}: ${href}`);
    }
  }
}

const home = await readFile(join(root, "index.html"), "utf8");
for (const href of [...expectedNavigationLinks, ...expectedProjectLinks]) {
  if (!home.includes(`href="${href}"`)) {
    throw new Error(`Pages verification did not find the expected navigation link: ${href}`);
  }
}
for (const marker of [
  "data-gw-documentation-header",
  "data-gw-documentation-search",
  "data-gw-project-switcher",
  "data-gw-theme-button",
]) {
  if (!home.includes(marker)) throw new Error(`Pages verification did not find the shared documentation control: ${marker}`);
}
if (!home.includes(">Docs</a>")) throw new Error("Pages verification did not find the single Docs header entry");
const header = home.match(/<header[^>]*data-gw-documentation-header[^>]*>[\s\S]*?<\/header>/)?.[0] || "";
for (const retired of [">Overview<", ">Get started<", ">Guides<", ">Reference<", ">Projects<", ">GitHub ↗<"]) {
  if (header.includes(retired)) throw new Error(`Pages verification found retired top-level navigation: ${retired}`);
}
if (!home.includes('href="https://github.com/greenways-ai/hoplite"')) {
  throw new Error("Pages verification did not find the front-page source action");
}
if (!home.includes("astro-code")) throw new Error("Pages verification did not find syntax-highlighted code output");
for (const copy of expectedHomepageCopy) {
  if (!home.includes(copy)) throw new Error(`Pages verification did not find the Hoplite product proof copy: ${copy}`);
}
for (const marker of expectedLaunchMarkers) {
  if (!home.includes(marker)) throw new Error(`Pages verification did not find the launch surface marker: ${marker}`);
}
for (const copy of forbiddenHomepageCopy) {
  if (home.includes(copy)) throw new Error(`Pages verification found retired homepage copy: ${copy}`);
}
for (const artwork of expectedAccentArtwork) {
  if (!home.includes(artwork)) throw new Error(`Pages verification did not find the accent artwork URL: ${artwork}`);
}

const benchmark = JSON.parse(await readFile(join(repositoryRoot, "src/data/http-benchmark.json"), "utf8"));
if (benchmark.status !== "measured") throw new Error("HTTP benchmark data has not been measured");
if (!Array.isArray(benchmark.samples) || benchmark.samples.length !== benchmark.rounds || benchmark.rounds < 3) {
  throw new Error("HTTP benchmark data does not contain the declared measured rounds");
}
for (const [name, value] of Object.entries(benchmark.metrics)) {
  if (!Number.isFinite(value) || value <= 0) throw new Error(`HTTP benchmark metric is not a positive measurement: ${name}`);
}
for (const label of ["requests / second", "p50 latency", "p99 latency", "peak container memory"]) {
  if (!home.includes(label)) throw new Error(`Pages verification did not render the raw benchmark label: ${label}`);
}

const lightModeCss = await readFile(join(repositoryRoot, "src/styles/documentation-enhancements.css"), "utf8");
if (!lightModeCss.includes("--gw-header: rgba(255, 255, 255, .97)")) {
  throw new Error("Hoplite light-mode documentation header is not using the white content surface");
}
if (!lightModeCss.includes(".hoplite-hero__copy")) {
  throw new Error("Hoplite light-mode hero does not define its contrast surface");
}

const installation = await readFile(join(root, installationPath), "utf8");
for (const marker of [
  "brew install greenways-ai/tap/hoplite",
  "scripts/install.sh",
  "scripts/new-app.sh",
  "ghcr.io/greenways-ai/hoplite:latest",
]) {
  if (!installation.includes(marker)) throw new Error(`Pages verification did not find the published installation path: ${marker}`);
}
if (installation.includes("## Build from source")) {
  throw new Error("Pages verification found the retired source-first installation section");
}

const legacyRuntimeModel = await readFile(join(root, legacyRuntimeModelPath), "utf8");
if (
  !legacyRuntimeModel.includes(`content="0; url=${expectedRuntimeModelUrl}"`) ||
  !legacyRuntimeModel.includes(`href="${expectedRuntimeModelUrl}"`)
) {
  throw new Error(`Pages verification did not find the legacy runtime model redirect to ${expectedRuntimeModelUrl}`);
}

const guide = await readFile(join(root, "guides/writing-web-services/index.html"), "utf8");
if (!guide.includes(expectedGuideUrl)) {
  throw new Error(`Pages verification did not find the canonical guide URL: ${expectedGuideUrl}`);
}

console.log(
  `Verified ${files.length} Pages documents under /hoplite, including shared documentation controls, centered search, project switching, syntax-highlighted examples, measured HTTP evidence, white light-mode navigation, published installation paths, accent artwork, and canonical redirects.`,
);
