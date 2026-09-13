# Kronika

[English version](README.md)

Kronika сохраняет метрики Linux, статистику PostgreSQL, планы запросов и
события из журналов PostgreSQL/PgBouncer. Сборщик работает на наблюдаемой машине
Linux либо записывает только данные PostgreSQL с локального или удалённого сервера.
Веб-интерфейс показывает нагрузку, процессы, запросы и блокировки за выбранный
час и их изменения во времени.

![Использование CPU и значения показателей процессов за записанный час](docs/images/processes.png)

[Открыть интерактивный пример](https://pgtoolz.github.io/Kronika/).

Запись за 5 сентября 2026 года, 19:00–20:00 UTC:
[Processes](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=processes) · [Statements](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=pg.statements) · [Plans](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=pg.plans) · [Host](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=host).

## Установка и запуск

[Установите Kronika 1.1.0](INSTALL.ru.md) из [архива для Linux](docs/releases.ru.md#download)
или [соберите из исходников](docs/build.ru.md).

Выберите `local` для метрик Linux и, при необходимости, PostgreSQL в той же VM
или pod. Режим `postgresql` подходит для удалённого сервера или сбора только
метрик базы данных. Локальные примеры используют `/var/lib/kronika`; сборщик
создаёт каталог, если его ещё нет.

### Linux и при необходимости PostgreSQL

Запустите сбор метрик Linux:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

<a id="linux-и-postgresql"></a>
Для PostgreSQL на машине сборщика укажите строку подключения в `KRONIKA_PG_DSN`
при запуске. Используйте учётную запись PostgreSQL с
[правами для сбора данных](INSTALL.ru.md#5-postgresql).

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

Для этого локального запуска не задавайте `KRONIKA_POSTGRES_EFFECTIVE_CPUS`:
число CPU берётся из записанных снимков машины.

### Только PostgreSQL — локальный или удалённый сервер

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika

KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

Если число CPU PostgreSQL известно, добавьте `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`,
заменив `4` нужным числом.
Без него SQL-метрики доступны, а PostgreSQL Health неизвестен. Подробнее — в [настройках сборщика](bins/kronika-collector/README.ru.md#remote-postgresql).

Чтобы собирать данные нескольких серверов PostgreSQL, запустите для каждого
из них процесс `kronika-collector` со своим DSN и отдельным каталогом хранения. См.
[пример двух серверов](bins/kronika-collector/README.ru.md#several-postgresql-servers).

### Открыть веб-интерфейс

Запустите `kronika-web` во втором терминале с каталогом данных сборщика.

#### Для режима `local`

Для Linux укажите `KRONIKA_WEB_SOURCES=1`, как ниже; если также собирается
PostgreSQL, замените `1` на `3`:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

#### Для режима `postgresql`

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  KRONIKA_WEB_SOURCES=2 /usr/local/bin/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите. Веб-интерфейс показывает сохранённую
историю измерений и данные текущего сбора. `Ctrl+C` останавливает любой из
процессов; записи остаются на диске. [Настройка systemd](docs/services.ru.md) описывает запуск
обеих программ как служб и изменение настроек уже работающей службы.

### Место на диске

Ориентир для PostgreSQL с примерно 500 таблицами и 3000 индексами —
**около 200 MB сжатых записей в сутки**. Объём зависит от интервалов сбора,
числа записываемых объектов и уникальных запросов.

`KRONIKA_RETENTION=2147483648` задаёт бюджет хранения **2 GiB** по умолчанию,
включая журналы и индексы. При превышении целевого объёма сборщик автоматически
удаляет самые старые завершённые записи вместе с их индексами.
Для **10 GiB** задайте `KRONIKA_RETENTION=10737418240` (значение в байтах).

`auto` и `auto:P` вместо фиксированного объёма задают целевую долю занятого места
на всей файловой системе хранилища. Правила ротации и автоматический режим —
в [настройках хранения](bins/kronika-collector/README.ru.md#storage).

## Данные и представления

| Область | Что можно посмотреть | Справочник |
| --- | --- | --- |
| Processes — процессы | Команда, состояние и номер процесса (PID), использование CPU, память, чтение и запись на диск; дерево процессов и активность за час. | [Метрики Linux](docs/metrics-linux.ru.md) |
| Host — система | CPU, память, ожидание ресурсов (PSI), сеть и диски, свободное место и связи устройств; лимиты и использование ресурсов cgroup. | [Метрики Linux](docs/metrics-linux.ru.md) |
| Overview, Activity, Locks, Vacuum — работа PostgreSQL | Общая нагрузка, сеансы и ожидания, цепочки блокировок, длительность запросов и транзакций, ход очистки таблиц. | [Метрики PostgreSQL](docs/metrics-postgresql.ru.md) |
| Statements и Plans — запросы и планы | Число вызовов, время выполнения и планирования, чтение страниц и временных файлов, запись журнала WAL, текст SQL и плана. | [Метрики PostgreSQL](docs/metrics-postgresql.ru.md) |
| Databases, Tables, Indexes — объекты PostgreSQL | Настройки, размеры, чтение и изменение данных, обслуживание и возраст транзакций; объединение объектов по базе, схеме и табличному пространству. | [Метрики PostgreSQL](docs/metrics-postgresql.ru.md) |
| Events — события | Группы похожих сообщений PostgreSQL/PgBouncer, время, длительность и записанный контекст событий; отметки метрик. | [Управление интерфейсом](docs/features.ru.md) |
| Время и графики | Выбор часа и момента внутри него, изменение показателей, карты активности, итоговые значения и распределение измерений. | [Время и вычисления](docs/metrics-time.ru.md) |

[Руководство по интерфейсу](docs/features.ru.md) описывает выбор показателей,
группировку, поиск, сортировку, просмотр подробностей (Inspector), графики
и экспорт. В [руководстве оператора](docs/operator-guide.ru.md) — четыре
примера с расчётами по записи выше.

![Записанный запрос, текст SQL и активность за интервал](docs/images/statements.png)

![Записанный план выполнения и связанный SQL](docs/images/plans.png)

## Сбор и доступ

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-ru-dark.svg">
  <img alt="Сборщик получает данные Linux и PostgreSQL; веб-сервер передаёт запись браузеру и MCP-клиентам" src="docs/images/architecture-ru.svg">
</picture>

Интервалы сбора по умолчанию: процессы — 5 секунд, основные метрики Linux —
10 секунд, метрики PostgreSQL — 30 секунд, таблицы и индексы — 300 секунд.
[Настройки сборщика](bins/kronika-collector/README.ru.md) описывают источники
данных, интервалы, права доступа и удаление старых записей.

Веб-сервер обслуживает браузер, HTTP API и MCP на одном адресе и порту.
MCP — протокол, через который ИИ-клиент может читать сохранённые данные.
Панель **AI** содержит настройки такого подключения.
[Инструменты MCP](docs/features.ru.md#mcp) возвращают значения на выбранный
момент, списки объектов по величине показателя, описания полей, события
и подробности строк.

## Переносимый HTML-экспорт

**Export** сохраняет выбранный интервал вашей записи в один интерактивный
HTML-файл. Он содержит интерфейс и данные: таблицы, поиск и графики работают
без сервера и сетевого подключения.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-ru-dark.svg">
  <img alt="Сохранение интервала записи в интерактивный HTML-файл для просмотра без сервера" src="docs/images/report-export-ru.svg">
</picture>

[kronika-dump](bins/kronika-dump/README.ru.md) читает хранилище и извлекает
интервал в отдельный файл записи ZMS; [kronika-report](bins/kronika-report/README.ru.md) преобразует
ZMS в HTML.

## Документация

- Настройка: [Установка](INSTALL.ru.md) · [Архивы и CI](docs/releases.ru.md) · [Сервисы](docs/services.ru.md) · [Сбои хранения](docs/storage-recovery.ru.md) · [Сборка](docs/build.ru.md)
- Справочники: [Интерфейс](docs/features.ru.md) · [Время](docs/metrics-time.ru.md) · [Linux](docs/metrics-linux.ru.md) · [PostgreSQL](docs/metrics-postgresql.ru.md) · [MCP](docs/mcp-clients.ru.md)
- Программы: [Сборщик](bins/kronika-collector/README.ru.md) · [Веб-сервер](bins/kronika-web/README.ru.md) · [Dump](bins/kronika-dump/README.ru.md) · [Report](bins/kronika-report/README.ru.md)
- Записанные поля: [Linux](docs/type-registry/os.ru.md) · [Метрики PostgreSQL](docs/type-registry/postgresql-metrics.ru.md) · [События PostgreSQL](docs/type-registry/postgresql.ru.md) · [События PgBouncer](docs/type-registry/pgbouncer.ru.md)
- Разработка: [Формат сегмента](crates/kronika-format/README.ru.md) · [Демонстрационная нагрузка для разработки](bins/kronika-demo/README.ru.md)

[Лицензия MIT](LICENSE).
