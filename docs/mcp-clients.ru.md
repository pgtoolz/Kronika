# Подключение MCP-клиентов

[English version](mcp-clients.md)

Через MCP AI-клиент может читать сохранённые данные Kronika: снимки, историю
показателей и строки записи. Подключение обслуживает уже запущенный
`kronika-web` по адресу `/mcp`.

Он принимает `POST /mcp` по протоколу Streamable HTTP, отвечает в JSON и
предоставляет только инструменты MCP. Сервер не хранит сессии MCP. При
включённой аутентификации каждый запрос передаёт заголовок
`Authorization: Basic <BASE64>` с теми же именем и паролем, что у веб-интерфейса.
`KRONIKA_WEB_AUTH=disabled` отключает проверку, но переменные
`KRONIKA_WEB_USER` и `KRONIKA_WEB_PASSWORD` остаются обязательными.
Запросы с заголовком `Origin` и адреса со строкой параметров после `?` отклоняются.

## Параметры подключения

Подставьте свои значения вместо обозначений в угловых скобках:

| Значение | Что подставить |
| --- | --- |
| `<URL>` | Адрес MCP, например `http://127.0.0.1:8080/mcp`. |
| `kronika` | Имя, под которым клиент запомнит сервер. |
| `<USER>`, `<PASSWORD>` | Значения `KRONIKA_WEB_USER`, `KRONIKA_WEB_PASSWORD`. |
| `<BASE64>` | Строку `<USER>:<PASSWORD>`, закодированную в Base64 без перевода строки. |

```bash
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

Панель **Connect an AI agent** создаёт настройки для выбранного клиента.
Имя сервера составляется из имени крупнейшей базы в записи и адреса подключения,
например `kronika-billing-192-168-0-22-8080`. При отключённой аутентификации
заголовок с учётными данными не добавляется.

## Claude Code

Чтобы подключение было доступно во всех ваших проектах:

```bash
claude mcp add --transport http --scope user kronika '<URL>' \
  --header 'Authorization: Basic <BASE64>'
```

Для одного проекта сохраните настройки в `.mcp.json`:

```json
{
  "mcpServers": {
    "kronika": {
      "type": "http",
      "url": "<URL>",
      "headers": {
        "Authorization": "Basic <BASE64>"
      }
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
http_headers = { "Authorization" = "Basic <BASE64>" }
```

## Cursor

Добавьте запись в `.cursor/mcp.json` проекта или в `~/.cursor/mcp.json` для
всех проектов:

```json
{
  "mcpServers": {
    "kronika": {
      "url": "<URL>",
      "headers": {
        "Authorization": "Basic <BASE64>"
      }
    }
  }
}
```

[Справочник MCP](features.ru.md#mcp) описывает инструменты, параметры,
единицы измерения и результаты. Формирование настроек в исходном коде:
[генератор](../bins/kronika-web/ui/src/mcp-prompts.ts),
[панель подключения](../bins/kronika-web/ui/src/mcp-connect.tsx).
