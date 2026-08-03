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
const unscopedRootLink =
  /(href|src|srcset|action)="\/(?!hoplite(?:\/|"))/g;

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
  const match = source.match(unscopedRootLink);
  if (match) {
    throw new Error(`Pages verification found an unscoped root link in ${path}: ${match[0]}`);
  }
}

const guide = await readFile(
  join(root, "guides/writing-web-services/index.html"),
  "utf8",
);
if (!guide.includes(expectedGuideUrl)) {
  throw new Error(`Pages verification did not find the canonical guide URL: ${expectedGuideUrl}`);
}

console.log(
  `Verified ${files.length} Pages documents under /hoplite, including the web services guide.`,
);
