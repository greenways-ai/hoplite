import { access, readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const legacyRuntimeModelPath =
  "hopliteconcepts/runtime-model/index.html";
const legacyHostMarker = "data-gw-legacy-host-redirect";
const expectedRuntimeModelUrl = "/hoplite/concepts/runtime-model/";
const required = [
  "index.html",
  "guides/writing-web-services/index.html",
  legacyRuntimeModelPath,
];
const artworkBase =
  "https://oss.greenways.ai/visual-language/artwork/hoplite/";
const expectedArtwork = [
  "rabbit-courtyard",
  "open-gate",
  "branching-paths",
  "wind-arcade",
].flatMap((scene) => [
  `${artworkBase}${scene}-day.webp`,
  `${artworkBase}${scene}-night.webp`,
  `${artworkBase}${scene}-day-mobile.webp`,
  `${artworkBase}${scene}-night-mobile.webp`,
]);
const expectedGuideUrl =
  "https://oss.greenways.ai/hoplite/guides/writing-web-services";
const expectedNavigationLinks = [
  "/hoplite/",
  "/hoplite/getting-started/",
  "/hoplite/guides/writing-web-services/",
  "/hoplite/reference/cli/",
  "https://oss.greenways.ai/",
];
const unscopedRootLink =
  /(href|src|srcset|action)="\/(?!hoplite(?:\/|"))/;
const duplicatedScope =
  /(href|src|srcset|action)="\/hoplite\/hoplite(?:\/|[A-Za-z0-9_-])/;

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

for (const path of required) {
  await access(join(root, path));
}

const files = await htmlFiles(root);
if (files.length === 0) {
  throw new Error("Pages verification found no generated HTML files");
}

for (const path of files) {
  const source = await readFile(path, "utf8");
  if (!source.includes(legacyHostMarker)) {
    throw new Error(
      `Pages verification did not find the legacy-host redirect in ${path}`,
    );
  }
  const unscoped = source.match(unscopedRootLink);
  if (unscoped) {
    throw new Error(
      `Pages verification found an unscoped root link in ${path}: ${unscoped[0]}`,
    );
  }
  const duplicated = source.match(duplicatedScope);
  if (duplicated) {
    throw new Error(
      `Pages verification found a duplicated base path in ${path}: ${duplicated[0]}`,
    );
  }

  const links = [...source.matchAll(/href="([^"]+)"/g)].map((match) => match[1]);
  for (const href of links) {
    if (/^(?:mailto:|tel:|javascript:)/.test(href)) continue;
    const url = new URL(href, pageUrl(path));
    if (url.origin !== "https://oss.greenways.ai") continue;
    if (!url.pathname.startsWith("/hoplite")) continue;
    if (!(await localTarget(url.pathname))) {
      throw new Error(
        `Pages verification found a broken internal link in ${path}: ${href}`,
      );
    }
  }
}

const home = await readFile(join(root, "index.html"), "utf8");
for (const href of expectedNavigationLinks) {
  if (!home.includes(`href="${href}"`)) {
    throw new Error(
      `Pages verification did not find the expected navigation link: ${href}`,
    );
  }
}
for (const marker of ["data-hoplite-search-open", "data-hoplite-theme-toggle"]) {
  if (!home.includes(marker)) {
    throw new Error(`Pages verification did not find the compact header control: ${marker}`);
  }
}
if (!home.includes("Four expressions of air")) {
  throw new Error("Pages verification did not find the canonical four-scene Hoplite catalogue");
}
for (const artwork of expectedArtwork) {
  if (!home.includes(artwork)) {
    throw new Error(`Pages verification did not find the canonical artwork URL: ${artwork}`);
  }
}

const legacyRuntimeModel = await readFile(
  join(root, legacyRuntimeModelPath),
  "utf8",
);
if (
  !legacyRuntimeModel.includes(
    `content="0; url=${expectedRuntimeModelUrl}"`,
  ) ||
  !legacyRuntimeModel.includes(`href="${expectedRuntimeModelUrl}"`)
) {
  throw new Error(
    `Pages verification did not find the legacy runtime model redirect to ${expectedRuntimeModelUrl}`,
  );
}

const guide = await readFile(
  join(root, "guides/writing-web-services/index.html"),
  "utf8",
);
if (!guide.includes(expectedGuideUrl)) {
  throw new Error(
    `Pages verification did not find the canonical guide URL: ${expectedGuideUrl}`,
  );
}

console.log(
  `Verified ${files.length} Pages documents under /hoplite, including all internal links, compact navigation, the four-scene artwork set, the legacy runtime model redirect, and the web services guide.`,
);
