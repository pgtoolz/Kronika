# Установка на Linux

[English version](INSTALL.md) · [README](README.ru.md)

Для записи и просмотра истории нужны две программы: `kronika-collector`
собирает данные о машине, а `kronika-web` показывает их в браузере.
В архиве также есть `kronika-dump` для чтения и вырезания части записи и
`kronika-report` для создания HTML-отчёта.

Шаги 1–2 устанавливают опубликованный архив. Чтобы скомпилировать программы
самостоятельно, используйте [инструкцию сборки](docs/build.ru.md), затем перейдите
к [запуску сборщика](#3-запуск-сборщика). Для сбора PostgreSQL понадобится
строка подключения с правами мониторинга.

## 1. Скачивание и распаковка

В [текущем релизе](https://github.com/pgtoolz/Kronika/releases/latest)
доступны архивы и файлы контрольных сумм `.tar.gz.sha256`.
Команда `uname -m` покажет архитектуру вашей машины:

| `uname -m` | Обозначение в имени архива |
| --- | --- |
| `x86_64` | `x86_64-unknown-linux-musl` |
| `aarch64` | `aarch64-unknown-linux-musl` |

Скачайте, проверьте и распакуйте архив. Для ARM64 замените первую строку на
`target=aarch64-unknown-linux-musl`:

```sh
target=x86_64-unknown-linux-musl
version=1.0.1
archive="kronika-$version-$target.tar.gz"
release_url="https://github.com/pgtoolz/Kronika/releases/download/v$version"
curl -fLO "$release_url/$archive"
curl -fLO "$release_url/$archive.sha256"
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
```

## 2. Установка

Скопируйте четыре программы в `/usr/local/bin`:

```sh
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
```

## 3. Запуск сборщика

Для локального сбора Linux создайте каталог записи, доступный только root.
Для PostgreSQL-only пропустите этот шаг и укажите каталог, доступный вашему пользователю:

```sh
sudo install -d -m 0700 /var/lib/kronika
```

Выберите запуск ниже: только Linux, локальный PostgreSQL или только PostgreSQL.
Настройки читаются при запуске программы.

<a id="3-сбор-linux"></a>
### Только Linux

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Укажите обычный каталог, а не символическую ссылку. Права root позволяют читать
защищённые счётчики дискового ввода-вывода процессов и локальные журналы.
По умолчанию сборщик опрашивает процессы каждые 5 секунд, основные показатели
Linux — каждые 10 секунд. Когда возраст накопленной записи достигает
900 секунд, сборщик сохраняет её в готовый сжатый файл — сегмент. Большой
объём данных может завершить сегмент раньше. До этого веб-сервер уже может читать текущий журнал
`active.wal`. `Ctrl+C` останавливает сбор и сохраняет журнал; повторный запуск
той же команды продолжает запись в этот каталог.

Целевой объём хранения `KRONIKA_RETENTION` по умолчанию равен `2147483648` байт
(2 GiB). Для цели 10 GiB добавьте `KRONIKA_RETENTION=10737418240`.
Раздел [«Хранение»](bins/kronika-collector/README.ru.md#storage) описывает,
какие файлы учитываются и в каком порядке удаляются старые записи.

<a id="5-postgresql"></a>
### Linux и PostgreSQL

Используйте роль с правами мониторинга. Чтобы создать такую роль, выполните
команды в `psql` от администратора PostgreSQL:

```sql
CREATE ROLE kronika_monitor LOGIN;
\password kronika_monitor
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
```

Роль должна наследовать права `pg_monitor` и иметь право `CONNECT` к каждой
базе, из которой собираются данные. Права на расширения выдаются отдельно в
каждой базе; они перечислены в разделе
[«Роль PostgreSQL»](bins/kronika-collector/README.ru.md#postgresql-role).

PostgreSQL на машине сборщика:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

| Параметр или подключение | Что он задаёт |
| --- | --- |
| `KRONIKA_COLLECTOR_MODE` | По умолчанию `local`: Linux и настроенный локальный PostgreSQL. `postgresql`: только PostgreSQL, без чтения OS, процессов и cgroup сборщика. |
| `KRONIKA_PG_DSNS` | Первый DSN включает метрики сервера. В режиме `local` все DSN, разделённые `;`, также ищут локальные журналы. Обязателен в режиме `postgresql`. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Необязательное целое `1..4294967295`: число CPU PostgreSQL. На общей локальной машине берётся автоматически из записи. Для удалённого сервера и контейнеров без явного значения число CPU неизвестно; SQL-метрики собираются. |
| Расширения | Поддерживаемые варианты `pg_stat_statements` и `pg_store_plans` обнаруживаются в доступных базах. Для Activity, Locks и статистики таблиц и индексов используются встроенные представления PostgreSQL. |
| Подключение | DSN принимает `sslmode=disable`, `prefer` (по умолчанию) или `require`; TLS проверяет CA и имя сервера. `KRONIKA_PG_SSL_ROOT_CERT` заменяет встроенные публичные CA сертификатами из PEM-файла. Прямое подключение и PgBouncer session pooling сохраняют нужные настройки сеанса. |
| Журналы | В режиме `local` `pg_current_logfile()` находит доступные локальные файлы; `KRONIKA_PG_LOGS` добавляет пути/шаблоны. В режиме `postgresql` читаются только явно заданные файлы `KRONIKA_PG_LOGS`. Удалённые файлы не скачиваются. Настройки журналов PgBouncer действуют только в `local`. |

Управляемый или удалённый PostgreSQL, без сбора метрик машины сборщика:

Для сбора только PostgreSQL и TLS ниже нужна [сборка из этой версии исходников](docs/build.ru.md); в опубликованных архивах 1.0.1 этих возможностей нет.

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR=./kronika-data \
  KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

Укажите доступный для записи каталог; права root для этого режима не нужны.
Если серверу доступны 4 CPU, добавьте `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`.
Иначе оставьте параметр незаданным: SQL-метрики доступны, PostgreSQL Health
неизвестен. См. [удалённый PostgreSQL](bins/kronika-collector/README.ru.md#remote-postgresql).

[Настройка сервисов](docs/services.ru.md) показывает, как хранить строки
подключения и пароль веб-сервера в файлах, доступных только root.
[Справочник сборщика](bins/kronika-collector/README.ru.md) описывает интервалы,
поддерживаемые версии расширений и форматы журналов.

<a id="4-запуск-web"></a>
## 4. Запуск веб-сервера

Во втором терминале задайте пароль и запустите веб-сервер с тем же каталогом
записи. Для сбора только Linux укажите `KRONIKA_WEB_SOURCES=1`, как в примере
ниже; для Linux вместе с PostgreSQL — `3`, для только PostgreSQL — `2`.
Укажите тот же доступный для записи каталог; для PostgreSQL-only `sudo` не нужен:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

### Веб-сервер для записи только PostgreSQL

```sh
KRONIKA_STORAGE_DIR=./kronika-data \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_AUTH=required \
  KRONIKA_WEB_USER=demo KRONIKA_WEB_PASSWORD=replace-with-password \
  KRONIKA_WEB_SOURCES=2 /usr/local/bin/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите. Веб-серверу нужен доступ на запись в тот же
каталог: он создаёт индексы `.idx` для быстрого поиска и файл блокировки,
который предотвращает одновременную перестройку индексов. В примере с `/var/lib/kronika` обе
программы работают от root с закрытым для других пользователей хранилищем.

`KRONIKA_WEB_SOURCES` сообщает, какие источники настроены; он не включает сбор
и не скрывает записанные данные. Имя пользователя и пароль обязательны даже
при `KRONIKA_WEB_AUTH=disabled`.

Чтобы открыть запись с другой машины, выполните на ней:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@monitored-host
```

Затем откройте на ней <http://127.0.0.1:8080/>. Подключение по SSH передаёт
запросы локальному веб-серверу наблюдаемой машины. ИИ-клиенты используют тот же
адрес и учётные данные, добавляя `/mcp`; [настройки подключения](docs/mcp-clients.ru.md)
также доступны в панели **AI**. [Руководство systemd](docs/services.ru.md)
описывает автоматический запуск обеих программ.

## Справочники

[Управление интерфейсом](docs/features.ru.md) · [Примеры исследования записи](docs/operator-guide.ru.md) ·
[Сборка из исходников](docs/build.ru.md) · [Чтение и вырезание записи](bins/kronika-dump/README.ru.md) ·
[HTML-отчёты](bins/kronika-report/README.ru.md)
