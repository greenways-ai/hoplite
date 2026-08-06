import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const prefix = "/hoplite";
const malformedPrefix =
  /(href|src|srcset|action)="\/hoplite(?!\/|")/;
const duplicatedPrefix =
  /(href|src|srcset|action)="\/hoplite\/hoplite(?=\/|[A-Za-z0-9_-])/g;
const unscopedRootLink =
  /(href|src|srcset|action)="\/(?!hoplite(?:\/|"))/g;

async function visit(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) await visit(path);
    if (!entry.isFile() || !entry.name.endsWith(".html")) continue;

    const source = await readFile(path, "utf8");
    const malformed = source.match(malformedPrefix);
    if (malformed) {
      throw new Error(
        `Malformed Pages base path in ${path}: ${malformed[0]}`,
      );
    }

    // Astro and custom components can both apply the configured base path.
    // Collapse exactly one duplicate before adding the prefix to genuinely
    // unscoped root links, then fail if a duplicate still remains.
    const normalized = source.replace(duplicatedPrefix, `$1="${prefix}`);
    const scoped = normalized.replace(unscopedRootLink, `$1="${prefix}/`);
    const duplicated = scoped.match(duplicatedPrefix);
    if (duplicated) {
      throw new Error(
        `Duplicated Pages base path in ${path}: ${duplicated[0]}`,
      );
    }

    if (scoped !== source) await writeFile(path, scoped);
  }
}

await visit(root);
