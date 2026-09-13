# Установка на Linux

[English version](INSTALL.md) · [README](README.ru.md)

Для записи и просмотра истории нужны две программы: `kronika-collector`
собирает метрики Linux или PostgreSQL, а `kronika-web` показывает их в браузере.
В архиве также есть `kronika-dump` для чтения и вырезания части записи и
`kronika-report` для создания HTML-отчёта.

## 1. Скачивание и распаковка

Версия **1.1.0 ещё не выпущена**. Используйте архив сборки для этой ревизии:
[скачайте, проверьте и распакуйте его](docs/releases.ru.md#development-builds).
Либо [соберите и установите программы из исходников](docs/build.ru.md),
затем перейдите к [запуску сборщика](#3-запуск-сборщика).

В каталоге распакованного архива проверьте программу перед установкой:

```sh
./kronika-collector --version
```

Для примеров с новым `KRONIKA_PG_DSN` она должна показать `1.1.0`.

## 2. Установка

Скопируйте четыре программы в `/usr/local/bin`:

```sh
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
```

## 3. Запуск сборщика

Выберите режим: Linux и при необходимости PostgreSQL (`local`) либо только
PostgreSQL на локальном или удалённом сервере (`postgresql`). Настройки читаются
при запуске программы.

<a id="3-сбор-linux"></a>
### Linux и при необходимости PostgreSQL

Для режима `local` создайте каталог записи, доступный только root:

```sh
sudo install -d -m 0700 /var/lib/kronika
```

Запустите сбор метрик Linux:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Укажите обычный каталог, а не символическую ссылку. Права root позволяют читать
защищённые счётчики дискового ввода-вывода процессов и локальные журналы.
Процессы опрашиваются каждые 5 секунд, основные метрики Linux — каждые 10 секунд.

Сборщик периодически сохраняет сжатые сегменты. Веб-сервер читает новые данные
из `active.wal`, не дожидаясь готового сегмента.
`Ctrl+C` останавливает сбор и сохраняет журнал; для продолжения
запустите ту же команду.

Целевой объём хранения `KRONIKA_RETENTION` по умолчанию равен `2147483648` байт
(2 GiB). Для цели 10 GiB добавьте `KRONIKA_RETENTION=10737418240`.
Раздел [«Хранение»](bins/kronika-collector/README.ru.md#storage) описывает,
какие файлы учитываются и в каком порядке удаляются старые записи.

<a id="5-postgresql"></a>
#### Подключение PostgreSQL

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

Для каждого сервера PostgreSQL, включая primary и standby, запустите отдельный
процесс `kronika-collector` со своим DSN и каталогом хранения. Бинарник для всех
процессов один. Каждый процесс собирает метрики доступных баз своего сервера;
отдельный сборщик для каждой базы не нужен. См.
[пример двух серверов](bins/kronika-collector/README.ru.md#several-postgresql-servers).

PostgreSQL и метрики Linux из той же VM или pod:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

На общей с PostgreSQL машине число CPU определяется автоматически;
оставьте `KRONIKA_POSTGRES_EFFECTIVE_CPUS` незаданным. Установленные расширения
`pg_stat_statements` и `pg_store_plans` дают статистику запросов и планы.
Для Activity, Locks и статистики таблиц и индексов используются встроенные
представления PostgreSQL.

### Только PostgreSQL — локальный или удалённый сервер

Этот режим подходит для удалённого сервера или сбора без метрик Linux.
Для сбора только PostgreSQL sudo не нужен.

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

В этом примере TLS обязателен; проверяются сертификат и имя сервера.
Для частного центра сертификации задайте `KRONIKA_PG_SSL_ROOT_CERT=/path/to/ca.pem`.

Если число доступных серверу CPU известно, задайте его в
`KRONIKA_POSTGRES_EFFECTIVE_CPUS` (например, `4`). Если неизвестно, оставьте
параметр незаданным: SQL-метрики доступны, PostgreSQL Health неизвестен. См. [удалённый PostgreSQL](bins/kronika-collector/README.ru.md#remote-postgresql).

[Настройка сервисов](docs/services.ru.md) показывает, как хранить строки
подключения и пароль веб-сервера в файлах, доступных только root.
[Справочник сборщика](bins/kronika-collector/README.ru.md) описывает интервалы,
поддерживаемые версии расширений и форматы журналов.

<a id="4-запуск-web"></a>
## 4. Запуск веб-сервера

Во втором терминале задайте пароль и запустите веб-сервер с тем же каталогом
записи.

### Для режима `local`

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

### Для режима `postgresql`

```sh
KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  KRONIKA_WEB_SOURCES=2 /usr/local/bin/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите. Веб-серверу нужен доступ на запись в тот же
каталог для создания поисковых индексов `.idx`. В примере с `/var/lib/kronika`
обе программы работают от root; хранилище недоступно другим пользователям.

`KRONIKA_WEB_SOURCES` сообщает, какие источники настроены; он не включает сбор
и не скрывает записанные данные. Настройки входа описаны в
[справочнике веб-сервера](bins/kronika-web/README.ru.md).

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
