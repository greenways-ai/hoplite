import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const prefix = "/hoplite";

async function visit(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) await visit(path);
    if (!entry.isFile() || !entry.name.endsWith(".html")) continue;
    const source = await readFile(path, "utf8");
    const scoped = source.replace(/(href|src|srcset|action)="\/(?!hoplite(?:\/|"))/g, `$1="${prefix}/`);
    if (scoped !== source) await writeFile(path, scoped);
  }
}

await visit(root);
