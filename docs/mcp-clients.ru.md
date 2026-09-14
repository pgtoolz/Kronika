# Подключение MCP-клиентов

[English version](mcp-clients.md)

Для подключения с Basic Auth скопируйте настройки клиента из панели **Connect an AI agent**.

## Параметры подключения

Вместо `<URL>` укажите `http://<server-ip>:8080/mcp` с IP-адресом веб-сервера.
Если сервер слушает только localhost, используйте [туннель SSH](../INSTALL.ru.md#4-запуск-web).

## Claude Code

Чтобы подключение было доступно во всех ваших проектах:

```bash
claude mcp add --transport http --scope user kronika '<URL>'
```

Для одного проекта сохраните настройки в `.mcp.json`:

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

Добавьте запись в `~/.codex/config.toml` для всех проектов или в
`.codex/config.toml` проекта, которому вы доверяете:

```toml
[mcp_servers.kronika]
url = "<URL>"
```

## Cursor

Добавьте запись в `.cursor/mcp.json` проекта или в `~/.cursor/mcp.json` для
всех проектов:

```json
{
  "mcpServers": {
    "kronika": {
      "url": "<URL>"
    }
  }
}
```

[Инструменты и параметры MCP](features.ru.md#mcp).
