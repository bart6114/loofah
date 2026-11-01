# Errors

## Meeting not found

List or search meetings again and use the returned ID. Do not retry a guessed ID.

## Vault not found

Run `loof --json doctor`. Ask the user to open Loofah once if the vault does not exist. If they keep data in a custom location, use `--vault-path DIR` or `LOOFAH_VAULT_PATH` after they provide the path.

## Vault operation failed

Confirm the desktop app and CLI come from compatible revisions. Do not repair or rewrite vault files from the agent.

## Export output exists

Choose a new path. Pass `--force` only when the user explicitly approves replacing that exact file.

## MCP server exits

Run `loof --json sessions list` to distinguish vault access from client configuration. Confirm the MCP command is `loof` and its only required argument is `mcp`.

With `--json`, errors contain `schema_version` and an `error` object with `code`, `message`, and `exit_code`. CLI exit codes are `1` for an operation failure, `2` for missing data, `3` for a missing vault, and `4` for an existing export target. Invalid CLI arguments use Clap's exit code and the `invalid_arguments` error code.

Missing or incompatible caches initialize automatically. For `cache_not_ready` or `cache_corrupt`, run `loof --json init` to repair. Retry `cache_busy` after the other cache operation finishes. For `write_persisted: true`, do not repeat the write; resolve the reported issue and refresh the cache.
