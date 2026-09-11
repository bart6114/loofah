# Setup

The CLI is the recommended interface for agents with shell access. The Loofah skill supplies instructions; the `loof` executable must also be available in the agent's environment.

## CLI

See the [CLI installation docs](https://loofah.io/installation/) for current installation options and PATH setup.

On macOS, the desktop app includes the CLI. Open **Settings > Agents**, then click **Install** in the **CLI** section. Use the command name shown there: stable builds use `loof`; development and staging builds use their own commands.

Verify the installation with `loof --version`, then run `loof --json doctor` to check vault access (substitute the command shown in the app when using another build).

Alternatively, install the standalone prebuilt binary on macOS or Linux:

```bash
curl -fsSL https://loofah.io/install.sh | bash
```

Or install from source:

```bash
git clone https://github.com/bart6114/loofah.git
cd loofah
cargo install --locked --path apps/cli
loof --version
```

Run the Loofah desktop app at least once so its local vault exists. Homebrew and Windows binary distribution are planned but not yet available.

## Locate and verify the vault

Run `loof --json doctor` to discover the configured local vault and check access. Read the resolved vault path and `ready` flag from its response. The CLI automatically follows the app's configured vault location, including a relocated vault; keep using that default when it is ready.

If discovery fails or the user wants a different vault, ask them for its path. If no vault exists yet, ask them to open Loofah once. Verify a user-provided path with:

```bash
loof --json --vault-path /path/to/vault doctor
```

Use `--vault-path DIR` or `LOOFAH_VAULT_PATH` for subsequent commands targeting that vault. Do not guess paths, search the filesystem for vaults, or read vault files directly.

## Optional MCP connection

Use MCP for read-only access when the client cannot run CLI commands or the user requests it. It is not required for the CLI workflow. The same `loof` executable provides the local stdio server:

```bash
loof mcp
```

A generic client configuration is:

```json
{
  "mcpServers": {
    "loofah": {
      "command": "loof",
      "args": ["mcp"]
    }
  }
}
```

Restart the client after changing its MCP configuration.
