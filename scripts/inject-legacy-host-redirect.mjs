import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const marker = "data-gw-legacy-host-redirect";
const legacyHosts = ["hoplite.greenways.ai", "www.hoplite.greenways.ai"];
const canonicalOrigin = "https://oss.greenways.ai";
const canonicalBase = "/hoplite";
const redirectScript = `<script ${marker}>(()=>{const h=new Set(${JSON.stringify(legacyHosts)});if(!h.has(window.location.hostname))return;const b=${JSON.stringify(canonicalBase)};const p=window.location.pathname||"/";const t=p==="/"||p===b?b+"/":p.startsWith(b+"/")?p:b+(p.startsWith("/")?p:"/"+p);window.location.replace(${JSON.stringify(canonicalOrigin)}+t+window.location.search+window.location.hash)})();</script>`;

let htmlCount = 0;
let injectedCount = 0;

async function visit(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await visit(path);
      continue;
    }
    if (!entry.isFile() || !entry.name.endsWith(".html")) continue;

    htmlCount += 1;
    const source = await readFile(path, "utf8");
    if (source.includes(marker)) continue;
    if (!/<head(?:\s[^>]*)?>/i.test(source)) {
      throw new Error(`Cannot inject the legacy-host redirect into ${path}: missing <head>.`);
    }

    const output = source.replace(
      /<head(?:\s[^>]*)?>/i,
      (head) => `${head}\n${redirectScript}`,
    );
    await writeFile(path, output);
    injectedCount += 1;
  }
}

await visit(root);
if (htmlCount === 0) throw new Error(`No HTML documents found under ${root}.`);
console.log(
  `Legacy-host redirect present in ${htmlCount} HTML documents (${injectedCount} updated).`,
);
