# MCP client configuration

[Русская версия](mcp-clients.ru.md)

Copy the client configuration from Kronika’s **Connect an AI agent** panel.

## Connection parameters

Replace `<URL>` with `http://<server-ip>:8080/mcp`, using the web server’s IP address.
For a server listening only on localhost, use an [SSH tunnel](../INSTALL.md#4-start-web).
If the server requires a password, add the [authentication header](#authentication).

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

## Authentication

`<USER>` and `<PASSWORD>` are the server’s [web credentials](../bins/kronika-web/README.md#configuration).
Compute `<BASE64>` with:

```bash
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

Add the header for your client:

- Claude Code command: `--header 'Authorization: Basic <BASE64>'`.
- JSON server object: `"headers": { "Authorization": "Basic <BASE64>" }`.
- TOML server entry: `http_headers = { "Authorization" = "Basic <BASE64>" }`.

[MCP tools and parameters](features.md#mcp).
