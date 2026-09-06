# Класс 2: события логов PgBouncer

[English version](pgbouncer.md)

Диапазон типов — `2_100_001`–`2_199_999`; секция `pgbouncer_events` содержит одну строку на распознанное событие. [Код формата записей](../../crates/kronika-registry/src/codec/pgbouncer_events.rs) определяет поля; [справочник Events](../features.ru.md#events) — группировку в интерфейсе.

## Источник и поля

По подключениям из `KRONIKA_PGBOUNCER_DSNS` сборщик узнаёт путь `logfile` через `SHOW CONFIG`. Пути и шаблоны из `KRONIKA_PGBOUNCER_LOGS` добавляют файлы явно. Файлы должны быть доступны для чтения на машине сборщика и иметь формат, описанный ниже. Источник собирает события логов; `SHOW POOLS`, `SHOW STATS`, `SHOW CLIENTS` и строки `stats:` не собираются.

| Поле | Допускает null | Определение |
| --- | --- | --- |
| `ts` | нет | Время строки, микросекунды Unix. |
| `source_file` | нет | Имя прочитанного файла; идентификатор источника. |
| `level` | нет | `0` FATAL, `1` ERROR, `2` WARNING, `3` LOG, `4` DEBUG, `5` NOISE. |
| `database` | да | Имя секции базы в `pgbouncer.ini`, взятое из части строки с описанием соединения. |
| `username` | да | Пользователь или записанное имя peer из части строки с описанием соединения. |
| `host` | да | Адрес клиента/сервера без порта. |
| `text` | нет | Нормализованное сообщение с продолжениями, до 5 KiB. |

## Формат и нормализация

```text
2026-08-07 12:34:56.789 MSK [12345] LOG C-0x55f1: db/user@10.0.0.1:41537 closing because: query timeout (age=42s)
```

| Компонент | Правило разбора |
| --- | --- |
| Время | Локальная зона хоста сборщика; обозначение зоны в строке пропускается. Дробная часть — до микросекунд, неоднозначный час — первое вхождение. |
| Описание соединения | Префикс `C-` или `S-`, затем `db/user@host:port`. Без этой части строки поля базы, пользователя и адреса отсутствуют. |
| `(nodb)`, `(nouser)` | Сохраняются буквальными значениями. |
| `peer-7@host:port` | `database=null`, `username="peer-7"`; адрес сохраняется без порта. |
| Адрес | Удаляется суффикс после последнего `:`; скобки IPv6 и `unix(<pid>)` сохраняются. |
| `closing because: ` | Обёртка и завершающее ` (age=Ns)` удаляются. |
| `pooler error: ` | Строка пропускается. |
| Продолжение | Строка с начальной табуляцией присоединяется к предыдущей записи. |

В примере `database="db"`, `username="user"`, `host="10.0.0.1"`, `text="query timeout"`. Отдельного поля `kind` нет.

## Распознаваемые префиксы

После снятия обёртки сообщение должно начинаться с одного из значений таблицы. Остальные записи пропускаются.

| Семейство | Точные префиксы |
| --- | --- |
| Подключение к серверу | `cannot connect`, `connect failed`, `server conn crashed?`, `server DNS lookup failed`, `server login failed`, `server login has been failing` |
| Ёмкость и вытеснение соединений | `evicted`, `bouncer resources exhaustion`, `out of memory`, `no memory for pool`, `no memory for authentication pool`, `too many servers in the pool`, `no more connections allowed (max_client_conn)`, `client connections exceeded (max_db_client_connections)`, `client connections exceeded (max_user_client_connections)` |
| Очередь и тайм-ауты | `query_wait_timeout`, `query_timeout`, `query timeout`, `idle transaction timeout`, `transaction timeout`, `cancel_wait_timeout`, `connect timeout`, `client_login_timeout`, `suspend_timeout` |
| Повторное использование соединения | `idle server got dirty`, `SV_IDLE server got dirty`, `SV_USED server got dirty`, `reset query failed`, `test query failed`, `exec_on_connect query failed`, `var change failed`, `invalid server parameter` |
| Процесс PgBouncer | `pooler is shutting down`, `client connections dropped, exiting`, `server connections dropped, exiting`, `accept() failed`, `cannot listen on`, `kernel file descriptor limit`, `process up`, `TLS configuration could not be reloaded, keeping old configuration`, `RELOAD Failed, see logs for more details` |
| Аутентификация и протокол | `password authentication failed`, `SASL authentication failed`, `LDAP authentication failed`, `PAM authentication failed`, `certificate authentication failed`, `no such user`, `no such database`, `broken auth file`, `error response from auth_query`, `unable to send auth_query`, `bad packet`, `bad pkt header`, `failed to parse packet`, `old V2 protocol not supported`, `TLS handshake error` |

Размеры буферов и сохранение позиции чтения: [чтение логов](postgresql.ru.md#границы-чтения). Источники: [разбор записей](../../crates/kronika-source-log/src/pgbouncer.rs), [префиксы](../../crates/kronika-source-log/src/pgbouncer/events.rs), [время](../../crates/kronika-source-log/src/timestamp.rs).
