import { access, readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const required = [
  "index.html",
  "guides/writing-web-services/index.html",
];
const expectedGuideUrl =
  "https://opensource.greenways.ai/hoplite/guides/writing-web-services";
const expectedNavigationLinks = [
  "/hoplite/getting-started/",
  "/hoplite/concepts/runtime-model/",
  "/hoplite/guides/production-operation/",
  "/hoplite/reference/cli/",
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

for (const path of required) {
  await access(join(root, path));
}

const files = await htmlFiles(root);
if (files.length === 0) {
  throw new Error("Pages verification found no generated HTML files");
}

for (const path of files) {
  const source = await readFile(path, "utf8");
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
}

const home = await readFile(join(root, "index.html"), "utf8");
for (const href of expectedNavigationLinks) {
  if (!home.includes(`href="${href}"`)) {
    throw new Error(
      `Pages verification did not find the expected navigation link: ${href}`,
    );
  }
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
  `Verified ${files.length} Pages documents under /hoplite, including navigation and the web services guide.`,
);
