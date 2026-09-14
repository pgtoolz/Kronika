# MCP client configuration

[Русская версия](mcp-clients.ru.md)

For connections with Basic Auth, copy the client settings from **Connect an AI agent**.

## Connection parameters

`<URL>`: `http://<server-ip>:8080/mcp`.
For a server listening only on localhost, use an [SSH tunnel](../INSTALL.md#4-start-web).

## Claude Code

To make the connection available in all your projects:

```bash
claude mcp add --transport http --scope user kronika '<URL>'
```

Project configuration in `.mcp.json`:

```json
{
  "mcpServers": {
    "kronika": {
      "type": "http",
      "url": "<URL>"
    }
  }
}
```

## Codex CLI

Entry in `~/.codex/config.toml` or a trusted project's `.codex/config.toml`:

```toml
[mcp_servers.kronika]
url = "<URL>"
```

## Cursor

Entry in the project's `.cursor/mcp.json` or `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "kronika": {
      "url": "<URL>"
    }
  }
}
```

[MCP tools and parameters](features.md#mcp).
