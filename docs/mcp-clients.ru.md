# Подключение MCP-клиентов

[English version](mcp-clients.md)

Скопируйте настройки клиента из панели **Connect an AI agent** в Kronika.

## Параметры подключения

Вместо `<URL>` укажите `http://<server-ip>:8080/mcp` с IP-адресом веб-сервера.
Если сервер слушает только localhost, используйте [туннель SSH](../INSTALL.ru.md#4-запуск-web).
Если сервер требует пароль, добавьте [заголовок авторизации](#авторизация).

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

## Авторизация

`<USER>` и `<PASSWORD>` — [имя и пароль веб-сервера](../bins/kronika-web/README.ru.md#конфигурация).
Получите `<BASE64>` командой:

```bash
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

Добавьте заголовок для своего клиента:

- В команду Claude Code: `--header 'Authorization: Basic <BASE64>'`.
- В объект сервера JSON: `"headers": { "Authorization": "Basic <BASE64>" }`.
- В запись сервера TOML: `http_headers = { "Authorization" = "Basic <BASE64>" }`.

[Инструменты и параметры MCP](features.ru.md#mcp).
