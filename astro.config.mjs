import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import mdx from "@astrojs/mdx";

export default defineConfig({
  site: "https://hoplite.greenways.ai",
  integrations: [
    starlight({
      title: "Hoplite",
      components: {
        ThemeProvider: "./src/components/GreenwaysThemeProvider.astro",
        ThemeSelect: "./src/components/GreenwaysThemeSelect.astro",
      },
      description: "A Hara application server built into Nginx.",
      logo: { src: "./public/favicon.svg", replacesTitle: false },
      favicon: "/favicon.svg",
      customCss: ["./src/styles/custom.css"],
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/greenways-ai/hoplite" },
      ],
      editLink: {
        baseUrl: "https://github.com/greenways-ai/hoplite/edit/main/",
      },
      lastUpdated: true,
      pagefind: true,
      tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 3 },
      sidebar: [
        { label: "Overview", slug: "index" },
        {
          label: "Getting started",
          items: [
            { label: "Introduction", slug: "getting-started" },
            { label: "Installation", slug: "getting-started/installation" },
            { label: "Your first application", slug: "getting-started/first-application" },
            { label: "Project configuration", slug: "getting-started/project-configuration" },
            { label: "Development workflow", slug: "getting-started/development-workflow" },
          ],
        },
        {
          label: "Concepts",
          items: [
            { label: "Applications & resources", slug: "concepts/applications-resources" },
            { label: "Requests & responses", slug: "concepts/requests-responses" },
            { label: "Runtime model", slug: "concepts/runtime-model" },
            { label: "Host capabilities", slug: "concepts/host-capabilities" },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Async handlers", slug: "guides/async-handlers" },
            { label: "Development console", slug: "guides/development-console" },
            { label: "Multiple applications", slug: "guides/multiple-applications" },
            { label: "Production operation", slug: "guides/production-operation" },
            { label: "OpenAPI output", slug: "guides/openapi" },
            { label: "Packaging", slug: "guides/packaging" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "CLI", slug: "reference/cli" },
            { label: "Project schema", slug: "reference/project-schema" },
            { label: "Build output", slug: "reference/build-output" },
            { label: "hoplite.core", slug: "reference/hoplite-core" },
            { label: "hoplite.dev", slug: "reference/hoplite-dev" },
            { label: "hoplite.internal", slug: "reference/hoplite-internal" },
          ],
        },
        {
          label: "Project",
          items: [
            { label: "Status & roadmap", slug: "project/status" },
            { label: "Contributing", slug: "project/contributing" },
            { label: "Source ↗", link: "https://github.com/greenways-ai/hoplite" },
            { label: "Greenways ↗", link: "https://greenways.ai/opensource/" },
            { label: "Hara ↗", link: "https://hara-lang.org" },
          ],
        },
      ],
      head: [
        { tag: "meta", attrs: { property: "og:image", content: "https://hoplite.greenways.ai/images/hoplite-phalanx.webp" } },
        { tag: "meta", attrs: { name: "twitter:card", content: "summary_large_image" } },
      ],
    }),
    mdx(),
  ],
});
