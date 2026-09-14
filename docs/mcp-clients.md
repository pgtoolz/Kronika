# MCP client configuration

[Русская версия](mcp-clients.ru.md)

MCP lets an AI client read saved Kronika snapshots, metric histories and recorded rows. The running `kronika-web` serves this connection at `/mcp`.

`POST /mcp` uses Streamable HTTP, returns JSON and provides tools without MCP sessions.
It uses the [web credentials](../bins/kronika-web/README.md#configuration):
leave both unset for access without authentication. When both are nonempty,
send `Authorization: Basic <BASE64>` with each request.
Requests with `Origin` or a query string are rejected.

## Connection parameters

Replace the angle-bracket placeholders with your connection details. The URL
must be reachable from the machine running the MCP client. For a web server
listening only on localhost, you can use the [SSH tunnel](../INSTALL.md#4-start-web)
from the install guide.

| Value | Definition |
| --- | --- |
| `<URL>` | Endpoint URL, for example `http://<server-ip>:8080/mcp`. Replace `<server-ip>` with the server's address. |
| `kronika` | Server name in the client configuration. |
| `<USER>`, `<PASSWORD>` | Values of `KRONIKA_WEB_USER`, `KRONIKA_WEB_PASSWORD`, if configured. |
| `<BASE64>` | For authentication only: Base64 encoding of `<USER>:<PASSWORD>` without a trailing newline. |

When credentials are configured, compute `<BASE64>` with:

```bash
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

The **Connect an AI agent** panel generates configuration for the selected client. The server name combines the largest database name in the recording with the connection address, for example `kronika-billing-192-168-0-22-8080`.

The examples below connect without authentication. When both server credentials
are configured, add `--header 'Authorization: Basic <BASE64>'` to the Claude Code
command, `"headers": { "Authorization": "Basic <BASE64>" }` to the server's JSON
object, or `http_headers = { "Authorization" = "Basic <BASE64>" }` to its TOML entry.

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

Tool inventory, parameter types, units and results: [MCP reference](features.md#mcp). Configuration sources: [generator](../bins/kronika-web/ui/src/mcp-prompts.ts), [connection panel](../bins/kronika-web/ui/src/mcp-connect.tsx).
