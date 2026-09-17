# Loofah documentation

This Astro Starlight project holds the Loofah docs, published
at https://loofah.io via a Cloudflare Worker serving static
assets.

## Local preview

```bash
pnpm install
pnpm -F @hypr/docs dev
```

Content pages live in `src/content/docs/`. Update the `sidebar` in
`astro.config.mjs` whenever a page is added, moved, or removed. Keep CLI and
MCP reference content aligned with `apps/cli/src/cli.rs` and
`apps/cli/src/mcp.rs`.

## Build and deploy

```bash
pnpm -F @hypr/docs build     # static output in docs/dist/
pnpm -F @hypr/docs deploy    # build + wrangler deploy (needs Cloudflare auth)
```

Pushes to `main` that touch `docs/` deploy automatically through
`.github/workflows/docs_deploy.yaml` (requires the `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` repository secrets). The canonical Worker
(`loofah-docs`) is configured in `wrangler.jsonc`.
`wrangler.redirect.jsonc` keeps the old domains and `www.loofah.io`
redirecting to `https://loofah.io` while preserving paths and query strings.

## Cloudflare access

Use the repository's installed Wrangler from the repository root:

```bash
pnpm cloudflare:login
pnpm cloudflare:whoami
pnpm -F @hypr/docs exec wrangler deployments list
```

The login command opens Cloudflare's browser authorization flow. It requests
Workers, routes, D1, Queues, Email Sending and Turnstile access, plus account,
user and zone information, for website and backend administration. This grants
access; it does not provision resources or enable paid products. If a saved
login expires and cannot refresh, run the login command again.

If the browser callback times out, use `pnpm cloudflare:login --device` and
complete the device-code authorization instead.

Both Wrangler configurations pin the `bart6114` account that owns `loofah.io`
(`6adc50875d0ccd569fe6b0558742e7a0`). The zone ID is
`481fd8bab0dddfac498d6d0ec8a43596`. These identifiers are public configuration,
not credentials. CI also supplies `CLOUDFLARE_ACCOUNT_ID` through its repository
secret; it must match the configured account. For noninteractive access, supply a scoped
`CLOUDFLARE_API_TOKEN` through the environment or CI secret store. GitHub Actions
secrets cannot be read back into a local Wrangler session.

These commands inspect backend access without creating resources:

```bash
pnpm -F @hypr/docs exec wrangler d1 list
pnpm -F @hypr/docs exec wrangler r2 bucket list
pnpm -F @hypr/docs exec wrangler queues list
pnpm -F @hypr/docs exec wrangler email sending list loofah.io
```

A successful website deployment does not establish that the CI token can
provision backend resources. Check its permissions when adding each service.
If R2 reports error `10042`, activate R2 in the account's Cloudflare dashboard.
An Email Sending `Unauthorized` response is a separate access/setup failure;
having the OAuth scope alone does not verify that sending is available.
Keep API tokens and Worker secrets out of committed files; `.env*` and
`.dev.vars*` are ignored.
