# Установка на Linux

[English version](INSTALL.md) · [README](README.ru.md)

Для записи и просмотра истории нужны две программы: `kronika-collector`
собирает метрики Linux или PostgreSQL, а `kronika-web` показывает их в браузере.
В архиве также есть `kronika-dump` для чтения и вырезания части записи и
`kronika-report` для создания HTML-отчёта.

## 1. Скачивание и распаковка

Скачайте [архив 1.1.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.1.1)
для своей архитектуры. Команды ниже — для x86-64. Для ARM64 задайте
`target=aarch64-unknown-linux-musl`.

```sh
target=x86_64-unknown-linux-musl
archive="kronika-1.1.1-$target.tar.gz"
curl -fLO "https://github.com/pgtoolz/Kronika/releases/download/v1.1.1/$archive"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
```

[Состав архивов и контрольные суммы](docs/releases.ru.md#download). Для самостоятельной
сборки воспользуйтесь [инструкцией](docs/build.ru.md), затем перейдите к
[запуску сборщика](#3-запуск-сборщика).

## 2. Установка

Выполните в каталоге распакованного архива:

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

Процессы опрашиваются каждые 5 секунд, основные метрики Linux — каждые 10 секунд.

Веб-интерфейс показывает сохранённую историю измерений и данные текущего сбора.
`Ctrl+C` останавливает сбор. Для продолжения запустите ту же команду.

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
каждой базе. Они перечислены в разделе
[«Роль PostgreSQL»](bins/kronika-collector/README.ru.md#postgresql-role).

Для сбора с нескольких серверов PostgreSQL запустите `kronika-collector`
для каждого сервера, указав его `KRONIKA_PG_DSN` и отдельный каталог
`KRONIKA_STORAGE_DIR`. См.
[пример двух серверов](bins/kronika-collector/README.ru.md#several-postgresql-servers).

PostgreSQL и метрики Linux из той же VM или pod:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

На общей с PostgreSQL машине число CPU определяется автоматически.
Оставьте `KRONIKA_POSTGRES_EFFECTIVE_CPUS` незаданным. Установленные расширения
`pg_stat_statements` и `pg_store_plans` дают статистику запросов и планы.
Для Activity, Locks и статистики таблиц и индексов используются встроенные
представления PostgreSQL.

### Только PostgreSQL — локальный или удалённый сервер

Этот режим подходит для удалённого сервера или сбора без метрик Linux.

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika

KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

Если число доступных серверу CPU известно, задайте его в
`KRONIKA_POSTGRES_EFFECTIVE_CPUS` (например, `4`). Если неизвестно, оставьте
параметр незаданным: SQL-метрики доступны, PostgreSQL Health неизвестен. См. [удалённый PostgreSQL](bins/kronika-collector/README.ru.md#remote-postgresql).

[Настройка сервисов](docs/services.ru.md) показывает, как хранить строки
подключения и пароль веб-сервера в файлах, доступных только root.
[Справочник сборщика](bins/kronika-collector/README.ru.md) описывает интервалы,
поддерживаемые версии расширений и форматы журналов.

<a id="4-запуск-web"></a>
## 4. Запуск веб-сервера

Во втором терминале запустите веб-сервер с тем же каталогом записи.

### Для режима `local`

Для Linux укажите `KRONIKA_WEB_SOURCES=1`, как ниже. Если также собирается
PostgreSQL, замените `1` на `3`:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
  KRONIKA_WEB_SOURCES=1 \
  /usr/local/bin/kronika-web
```

### Для режима `postgresql`

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
  KRONIKA_WEB_SOURCES=2 /usr/local/bin/kronika-web
```

Откройте `http://<server-ip>:8080`.

Для входа по паролю добавьте `KRONIKA_WEB_USER=kronika` и
`KRONIKA_WEB_PASSWORD='replace-with-a-random-password'` в команду запуска.

Веб-серверу нужен доступ на запись в тот же каталог для создания поисковых
индексов `.idx`. В примере для режима `local`
обе программы работают от root. Хранилище недоступно другим пользователям.

`KRONIKA_WEB_SOURCES` сообщает, какие источники настроены. Он не включает сбор
и не скрывает записанные данные. Настройки входа описаны в
[справочнике веб-сервера](bins/kronika-web/README.ru.md).

Для локального доступа, обратного прокси на той же машине или перенаправления
порта по SSH укажите `KRONIKA_WEB_LISTEN=127.0.0.1:8080`. Этот же адрес
используется по умолчанию. Для перенаправления выполните на машине клиента:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@monitored-host
```

Затем откройте на ней <http://127.0.0.1:8080/>. Подключение по SSH передаёт
запросы локальному веб-серверу наблюдаемой машины. ИИ-клиенты используют тот же
адрес и настройки аутентификации, добавляя `/mcp`. [Настройки подключения](docs/mcp-clients.ru.md)
также доступны в панели **AI**. [Руководство systemd](docs/services.ru.md)
описывает автоматический запуск обеих программ.

## Справочники

[Управление интерфейсом](docs/features.ru.md) · [Примеры исследования записи](docs/operator-guide.ru.md) ·
[Сборка из исходников](docs/build.ru.md) · [Чтение и вырезание записи](bins/kronika-dump/README.ru.md) ·
[HTML-отчёты](bins/kronika-report/README.ru.md)
