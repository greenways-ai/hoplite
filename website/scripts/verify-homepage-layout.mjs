import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const websiteRoot = fileURLToPath(new URL("../", import.meta.url));
const distRoot = join(websiteRoot, "dist");
const homepagePath = join(distRoot, "index.html");
const homepage = await readFile(homepagePath, "utf8");

if (!homepage.includes("class=\"hoplite-hero\"")) {
  throw new Error("Homepage verification did not find the authored Hoplite hero");
}

for (const forbiddenShellMarker of [
  "data-has-sidebar",
  "sidebar-pane",
  "right-sidebar-container",
]) {
  if (homepage.includes(forbiddenShellMarker)) {
    throw new Error(`Splash homepage unexpectedly renders ${forbiddenShellMarker}`);
  }
}

for (const requiredHeaderMarker of [
  "data-gw-documentation-header",
  "data-gw-documentation-search",
  "data-gw-theme-button",
]) {
  if (!homepage.includes(requiredHeaderMarker)) {
    throw new Error(`Splash homepage is missing ${requiredHeaderMarker}`);
  }
}

const contentPanels = [...homepage.matchAll(/class="[^"]*\bcontent-panel\b[^"]*"/g)];
if (contentPanels.length !== 1) {
  throw new Error(`Expected one homepage content panel; found ${contentPanels.length}`);
}

const stylesheetHrefs = [...homepage.matchAll(/<link\b[^>]*>/g)]
  .map(([tag]) => {
    if (!/\brel="stylesheet"/.test(tag)) return null;
    return tag.match(/\bhref="([^"]+)"/)?.[1] ?? null;
  })
  .filter((href) => href?.startsWith("/hoplite/"));

if (stylesheetHrefs.length === 0) {
  throw new Error("Homepage verification found no scoped stylesheets");
}

const stylesheets = [];
for (const href of stylesheetHrefs) {
  const localPath = join(distRoot, href.replace(/^\/hoplite\//, ""));
  stylesheets.push(await readFile(localPath, "utf8"));
}

const css = stylesheets.join("\n").replace(/\/\*[\s\S]*?\*\//g, "");
const panelSelector = "main:has(.hoplite-hero)>.content-panel:first-child";
const escapedPanelSelector = panelSelector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const panelRules = [...css.matchAll(new RegExp(`${escapedPanelSelector}\\{([^}]*)\\}`, "g"))];

if (panelRules.length === 0) {
  throw new Error("Homepage styles do not declare the splash content-panel policy");
}

const displayDeclarations = panelRules.flatMap((rule, ruleIndex) =>
  [...rule[1].matchAll(/(?:^|;)display:([^;!}]+)(!important)?/g)].map((match, declarationIndex) => ({
    value: match[1].trim(),
    important: Boolean(match[2]),
    ruleIndex,
    declarationIndex,
  })),
);
const importantDisplays = displayDeclarations.filter((declaration) => declaration.important);
const effectiveDisplay = (importantDisplays.length > 0 ? importantDisplays : displayDeclarations).at(-1);

if (effectiveDisplay?.value !== "block") {
  throw new Error(
    `Homepage content panel resolves to display:${effectiveDisplay?.value ?? "<unset>"}${effectiveDisplay?.important ? " !important" : ""}; expected block`,
  );
}

const generatedHeroRule = /main:has\(\.hoplite-hero\)>\.content-panel>\.sl-container>\.hero\{[^}]*display:none/;
if (!generatedHeroRule.test(css)) {
  throw new Error("Homepage styles do not hide only Starlight's generated hero");
}

for (const requiredCopy of [
  "Your application, directly inside Nginx.",
  "Hoplite against raw Nginx.",
  "What each stack occupies.",
]) {
  if (!homepage.includes(requiredCopy)) {
    throw new Error(`Homepage content is missing: ${requiredCopy}`);
  }
}

console.log(
  `Verified the sidebar-free authored homepage remains inside its single visible content panel across ${stylesheetHrefs.length} stylesheets.`,
);
