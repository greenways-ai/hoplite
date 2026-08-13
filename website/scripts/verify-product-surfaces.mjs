import { access, readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const websiteRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../", import.meta.url));
const contentRoot = join(websiteRoot, "src/content/docs");
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const publicSurfaces = await readJson(join(repositoryRoot, "docs/public-surfaces.json"));
const coreBoundary = await readJson(join(repositoryRoot, "docs/core-boundary.json"));

if (publicSurfaces.format !== "hoplite.public-surfaces/0-alpha") throw new Error("Unexpected public-surface registry");
if (coreBoundary.format !== "hoplite.core-boundary/0-alpha" || coreBoundary.retirement?.decision !== "retired") throw new Error("Unexpected core-boundary retirement decision");

const retiredNames = new Set(["hoplite.auth", "hoplite.value", "hoplite.blob", "hoplite.store"]);
for (const section of Object.values(publicSurfaces)) {
  if (!Array.isArray(section)) continue;
  for (const entry of section) {
    if (entry.status === "migration-only") throw new Error(`Migration surface remains published: ${entry.name}`);
    if (retiredNames.has(entry.name)) throw new Error(`Retired product remains published: ${entry.name}`);
    if (entry.status !== "internal" && (!Array.isArray(entry.conformance) || entry.conformance.length === 0)) throw new Error(`Published surface lacks conformance: ${entry.name}`);
  }
}
for (const sectionName of ["provider_products", "migration_products"]) {
  if (coreBoundary[sectionName]?.length !== 0) throw new Error(`${sectionName} must remain empty after 0.2.0 retirement`);
}

const cliReference = await readFile(join(contentRoot, "reference/cli.mdx"), "utf8");
for (const command of publicSurfaces.cli_commands.filter(({ status }) => status === "public")) {
  const marker = command.program === "hoplite-server" ? "hoplite-server" : `hoplite ${command.name}`;
  if (!cliReference.includes(marker)) throw new Error(`CLI reference does not cover ${command.program} ${command.name}`);
}

const requiredMarkers = {
  "guides/diagnostics.md": ["hoplite.inspect/0-alpha", "hoplite.doctor/0-alpha", "without accidentally executing source"],
  "reference/build-output.md": ["Source-free production", "hoplite.development-source-projection/0-alpha", "retired in 0.2.0"],
  "concepts/data-plane-providers.md": ["Data-plane boundaries", "retired in 0.2.0"],
};
for (const [path, markers] of Object.entries(requiredMarkers)) {
  const source = await readFile(join(contentRoot, path), "utf8");
  for (const marker of markers) if (!source.includes(marker)) throw new Error(`${path} is missing ${marker}`);
}

async function documentationFiles(directory) {
  const output = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) output.push(...await documentationFiles(path));
    if (entry.isFile() && /\.(?:md|mdx)$/.test(entry.name)) output.push(path);
  }
  return output;
}

for (const path of await documentationFiles(contentRoot)) {
  const source = await readFile(path, "utf8");
  for (const match of source.matchAll(/https:\/\/github\.com\/greenways-ai\/hoplite\/blob\/main\/([^\s)`"#]+)/g)) {
    try { await access(join(repositoryRoot, decodeURIComponent(match[1]))); }
    catch { throw new Error(`${relative(repositoryRoot, path)} links to missing source: ${match[1]}`); }
  }
}

console.log("Verified website documentation against the retired-product and public-surface registries.");
