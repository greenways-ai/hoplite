import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import mdx from "@astrojs/mdx";

export default defineConfig({
  site: "https://oss.greenways.ai",
  base: "/hoplite",
  vite: { build: { assetsInlineLimit: 0 } },
  integrations: [
    starlight({
      title: "Hoplite",
      components: {
        Header: "./src/components/SharedSiteHeader.astro",
        ThemeProvider: "./src/components/GreenwaysThemeProvider.astro",
        ThemeSelect: "./src/components/GreenwaysThemeSelect.astro",
      },
      description: "A high-performance Hara application server built directly into Nginx.",
      logo: { src: "./public/sigil.svg", replacesTitle: false },
      favicon: "/hoplite/favicon.svg",
      customCss: [
        "./src/styles/custom.css",
        "./src/styles/starlight-shell.css",
        "./src/styles/documentation-enhancements.css",
        "./src/styles/benchmark-status.css",
      ],
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/greenways-ai/hoplite" },
      ],
      editLink: {
        baseUrl: "https://github.com/greenways-ai/hoplite/edit/main/website/",
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
            { label: "Data-plane providers", slug: "concepts/data-plane-providers" },
          ],
        },
        {
          label: "Hoplite from First Principles",
          items: [
            { label: "Book overview", slug: "first-principles" },
            { label: "1. Values and boundaries", slug: "first-principles/values-and-boundaries" },
            { label: "2. Inside a Hoplite worker", slug: "first-principles/worker-runtime" },
            { label: "3. Standard Hara", slug: "first-principles/standard-hara" },
            { label: "4. Streams", slug: "first-principles/streams" },
            { label: "5. Channels and stream.async", slug: "first-principles/stream-async" },
            { label: "6. Duplex and Relay", slug: "first-principles/duplex-relay" },
            { label: "7. Progressive case studies", slug: "first-principles/case-studies" },
            { label: "8. Application catalogue", slug: "first-principles/application-catalogue" },
            { label: "9. Performance", slug: "first-principles/performance" },
            { label: "10. Maintainability", slug: "first-principles/maintainability" },
            { label: "11. Production reasoning", slug: "first-principles/production" },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Writing web services", slug: "guides/writing-web-services" },
            { label: "Realtime channels & WebRTC", slug: "guides/realtime-channels-and-webrtc" },
            { label: "RTC streams & Relay", slug: "guides/rtc-stream-relay" },
            { label: "Async handlers", slug: "guides/async-handlers" },
            { label: "Development console", slug: "guides/development-console" },
            { label: "Multiple applications", slug: "guides/multiple-applications" },
            { label: "Production operation", slug: "guides/production-operation" },
            { label: "Diagnostics", slug: "guides/diagnostics" },
            { label: "Authentication", slug: "guides/authentication" },
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
            { label: "hoplite.nchan", slug: "reference/hoplite-nchan" },
            { label: "hoplite.rtc", slug: "reference/hoplite-rtc" },
            { label: "hoplite.host", slug: "reference/hoplite-host" },
            { label: "hoplite.response-source", slug: "reference/hoplite-response-source" },
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
            { label: "Greenways ↗", link: "https://oss.greenways.ai/" },
            { label: "Hara ↗", link: "https://hara-lang.org" },
          ],
        },
      ],
      head: [
        { tag: "meta", attrs: { property: "og:image", content: "https://oss.greenways.ai/visual-language/assets/og-hoplite.jpg" } },
        { tag: "meta", attrs: { property: "og:image:secure_url", content: "https://oss.greenways.ai/visual-language/assets/og-hoplite.jpg" } },
        { tag: "meta", attrs: { property: "og:image:type", content: "image/jpeg" } },
        { tag: "meta", attrs: { property: "og:image:width", content: "1200" } },
        { tag: "meta", attrs: { property: "og:image:height", content: "630" } },
        { tag: "meta", attrs: { property: "og:image:alt", content: "Hoplite's cyan compass-star sigil over the rabbit courtyard" } },
        { tag: "meta", attrs: { name: "twitter:image", content: "https://oss.greenways.ai/visual-language/assets/og-hoplite.jpg" } },
        { tag: "meta", attrs: { name: "twitter:image:alt", content: "Hoplite's cyan compass-star sigil over the rabbit courtyard" } },
        { tag: "meta", attrs: { name: "twitter:card", content: "summary_large_image" } },
      ],
    }),
    mdx(),
  ],
});
