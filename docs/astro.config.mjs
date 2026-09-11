import starlight from "@astrojs/starlight";
import { defineConfig } from "astro/config";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";

import { remarkChangelogBanner } from "./src/remark-changelog-banner.mjs";

export default defineConfig({
  site: "https://loofah.io",
  markdown: {
    remarkPlugins: [remarkChangelogBanner],
  },
  integrations: [
    starlight({
      title: "Loofah",
      description:
        "Free meeting transcription for Mac. Record without a bot, keep notes as Markdown, and connect agents to your local knowledge vault.",
      components: {
        PageTitle: "./src/components/PageTitle.astro",
      },
      head: [
        {
          tag: "meta",
          attrs: {
            property: "og:image",
            content: "https://loofah.io/screenshots/loofah-primary.png",
          },
        },
        {
          tag: "meta",
          attrs: {
            property: "og:image:alt",
            content:
              "Loofah showing meeting notes, a transcript, and sessions saved in a local vault",
          },
        },
        {
          tag: "meta",
          attrs: {
            name: "twitter:image",
            content: "https://loofah.io/screenshots/loofah-primary.png",
          },
        },
      ],
      logo: {
        src: "../apps/desktop/src-tauri/icons/src/loofah-mark-1024.png",
      },
      favicon: "/favicon.png",
      customCss: ["./src/styles/custom.css"],
      plugins: [
        starlightLinksValidator(),
        starlightLlmsTxt({
          promote: ["loofah", "agents/**", "reference/**", "installation"],
        }),
      ],
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/bart6114/loofah",
        },
      ],
      editLink: {
        baseUrl: "https://github.com/bart6114/loofah/edit/main/docs/",
      },
      sidebar: [
        {
          label: "Getting started",
          items: ["loofah", "is-it-free", "background", "quickstart"],
        },
        {
          label: "Using Loofah",
          items: [
            "meetings",
            "automatic-capture",
            "import-recordings",
            "notes",
            "syncing",
            "customize-summaries",
            "languages",
          ],
        },
        {
          label: "AI and privacy",
          items: ["ai-setup", "offline", "data-and-privacy"],
        },
        {
          label: "Compare meeting tools",
          items: [
            "compare",
            "compare/meetily",
            "compare/granola",
            "compare/otter",
          ],
        },
        {
          label: "CLI and agents",
          items: [
            "installation",
            "agents/overview",
            "agents/skills",
            "agents/cli",
            "agents/connect-clients",
            "agents/mcp",
            "agents/vault",
          ],
        },
        {
          label: "Reference",
          items: [
            "reference/cli",
            "reference/mcp",
            "reference/errors",
            "artwork",
          ],
        },
        {
          label: "Help",
          items: ["help", "troubleshooting", "changelog"],
        },
      ],
    }),
  ],
});
