# Loofah documentation instructions

## Scope

- This is an Astro Starlight project, published at
  https://loofah.io through a Cloudflare Worker serving the
  static build (`.github/workflows/docs_deploy.yaml`).
- Write for Loofah users, developers, and agents using the
  CLI or MCP server.
- Site configuration (sidebar, theme, plugins) lives in `astro.config.mjs`;
  content pages are MDX under `src/content/docs/`.
- The public agent skill is maintained in `../skills/loofah/`.

## Sources of truth

- Treat `apps/cli/src/cli.rs` as the CLI command contract.
- Treat `apps/cli/src/mcp.rs` as the MCP tool and resource contract.
- Treat current release automation as the source of truth for installation channels.
- Verify product behavior against the shared write path in `crates/vault-write/`, extracted operations in `crates/loofah-core/` and the relevant desktop/CLI/server adapter. Desktop session-store commands in `apps/desktop/src-tauri/src/session_store/` wrap the shared store. Do not infer supported behavior solely from raw vault files or schema definitions.

## Writing

- Use active voice and second person.
- Keep headings and sentences concise.
- Put the result before implementation detail.
- Use `Loofah` for the product and `loof` for the executable.
- Use root-relative links between docs pages (e.g. `/quickstart`).
- Use Starlight components: `:::note` / `:::caution` asides, and `Steps`,
  `CardGrid`, `LinkCard` from `@astrojs/starlight/components`.

## Accuracy boundaries

- Document only commands, options, tools, resources, and output behavior present in the source.
- Mark planned features and distribution channels as forthcoming.
- Never describe Homebrew or Windows binaries as available until release automation publishes them.
- Never tell agents to crawl or modify the vault files directly; agents go through the CLI or MCP server.
- Make it clear that CLI and MCP transcript commands return the complete transcript and can produce large responses.

## Verification

- Update the `sidebar` in `astro.config.mjs` after adding or moving a page.
- Run `pnpm exec dprint fmt docs skills` from the repository root.
- Run `pnpm exec dprint check docs skills` before submitting.
- Run `pnpm -F @hypr/docs build` before deploying — the build fails on broken
  internal links (starlight-links-validator) and invalid MDX.
