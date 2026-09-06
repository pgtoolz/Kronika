# Установка на Linux

[English version](INSTALL.md) · [README](README.ru.md)

Для записи и просмотра истории нужны две программы: `kronika-collector`
собирает данные о машине, а `kronika-web` показывает их в браузере.
В архиве также есть `kronika-dump` для чтения и вырезания части записи и
`kronika-report` для создания HTML-отчёта.

Выберите архив для архитектуры вашего Linux-хоста. Программам нужен доступ к
каталогу записи. Для сбора статистики PostgreSQL понадобится строка подключения
к серверу с правами мониторинга.

## 1. Скачивание и распаковка

[Скачайте архив сборки](docs/releases.ru.md#download) и файл контрольной суммы
`.tar.gz.sha256`. Команда `uname -m` покажет архитектуру вашей машины:

| `uname -m` | Обозначение в имени архива |
| --- | --- |
| `x86_64` | `x86_64-unknown-linux-musl` |
| `aarch64` | `aarch64-unknown-linux-musl` |

В каталоге загрузки подставьте имя скачанного файла. Команды ниже проверяют
контрольные суммы, распаковывают архив и показывают сведения о сборке:

```sh
archive='kronika-1.0.0-REPLACE_WITH_COMMIT-x86_64-unknown-linux-musl.tar.gz'
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
cat BUILDINFO
```

## 2. Установка

Проверьте версии четырёх программ и скопируйте их в `/usr/local/bin`:

```sh
for binary in kronika-collector kronika-web kronika-dump kronika-report; do
  "./$binary" --version
done
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
```

## 3. Сбор Linux

На машине, которую хотите наблюдать, создайте каталог записи и запустите
сборщик:

```sh
sudo install -d -m 0700 /var/lib/kronika
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

<a id="4-запуск-web"></a>
## 4. Запуск веб-сервера

Во втором терминале замените пароль и выполните:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите. Веб-серверу нужен доступ на запись в тот же
каталог: он создаёт индексы `.idx` для быстрого поиска и файл блокировки,
который предотвращает одновременную перестройку индексов. В этом примере обе
программы работают от root с закрытым для других пользователей хранилищем.

Чтобы открыть запись с другой машины, выполните на ней:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@monitored-host
```

Затем откройте на ней <http://127.0.0.1:8080/>. Подключение по SSH передаёт
запросы локальному веб-серверу наблюдаемой машины. AI-клиенты используют тот же
адрес и учётные данные, добавляя `/mcp`; [настройки подключения](docs/mcp-clients.ru.md)
также доступны в панели **AI**. [Руководство systemd](docs/services.ru.md)
описывает автоматический запуск обеих программ.

## 5. PostgreSQL

В `psql` от администратора PostgreSQL создайте роль для мониторинга:

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

Остановите сборщик через `Ctrl+C`. Если PostgreSQL работает в той же
виртуальной машине или в том же контейнере и использует те же ограничения
ресурсов, запустите сборщик без `KRONIKA_POSTGRES_EFFECTIVE_CPUS`:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

| Параметр или подключение | Что он задаёт |
| --- | --- |
| `KRONIKA_PG_DSNS` | Строки подключения (DSN), разделённые `;`. Первая включает сбор метрик из доступных баз этого сервера. Все строки, включая первую, используются для обнаружения журналов. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Необязательное целое `1..4294967295`: число CPU, доступных наблюдаемому PostgreSQL. Без явного значения используются записанные сведения о CPU виртуальной машины или контейнера сборщика. |
| Расширения | Поддерживаемые варианты `pg_stat_statements` и `pg_store_plans` обнаруживаются в доступных базах. Для Activity, Locks и статистики таблиц и индексов используются встроенные представления PostgreSQL. |
| Подключение | Клиент работает без TLS (`NoTls`). Допустимо прямое подключение к PostgreSQL или PgBouncer в режиме session pooling: одно серверное соединение закрепляется за сессией и сохраняет настройки `SET`. |
| Журналы | Каждая строка из `KRONIKA_PG_DSNS` автоматически находит текущий журнал через `pg_current_logfile()`, даже если `KRONIKA_PG_LOGS` не задана. Файл должен быть доступен для чтения на машине сборщика. `KRONIKA_PG_LOGS` добавляет локальные пути или шаблоны имён файлов. Для PgBouncer служат `KRONIKA_PGBOUNCER_DSNS` и `KRONIKA_PGBOUNCER_LOGS`. |

Если PostgreSQL удалённый или работает в другой cgroup — группе процессов с
общими ограничениями ресурсов, — укажите доступное ему число CPU явно.
Пример: у сборщика 8 CPU, а у PostgreSQL 4 CPU:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  KRONIKA_POSTGRES_EFFECTIVE_CPUS=4 \
  /usr/local/bin/kronika-collector
```

Без этого параметра расчёт предполагает общие ресурсы с сборщиком; адрес в
строке подключения не подтверждает это условие. Для виртуальной машины берётся
число CPU из последнего записанного снимка. Для контейнера учитываются квота
процессорного времени, её период и разрешённый набор CPU (`cpuset`): например,
квота `150000/100000` соответствует `1.5` CPU. Если эти сведения отсутствуют,
показатель PostgreSQL Health не вычисляется (`null`); известное число CPU
можно задать вручную. [Формулы и выбор времени](docs/metrics-time.ru.md#health).

Перезапустите веб-сервер с `KRONIKA_WEB_SOURCES=3`, чтобы отметить Linux и PostgreSQL
как настроенные источники. Этот параметр сообщает веб-серверу о настройке; сам сбор
включается строками подключения сборщика. Имя пользователя и пароль
обязательны даже при `KRONIKA_WEB_AUTH=disabled`.

[Настройка сервисов](docs/services.ru.md) показывает, как хранить строки
подключения и пароль веб-сервера в файлах, доступных только root.
[Справочник сборщика](bins/kronika-collector/README.ru.md) описывает интервалы,
поддерживаемые версии расширений и форматы журналов.

## Справочники

[Управление интерфейсом](docs/features.ru.md) · [Примеры исследования записи](docs/operator-guide.ru.md) ·
[Сборка из исходников](docs/build.ru.md) · [Чтение и вырезание записи](bins/kronika-dump/README.ru.md) ·
[HTML-отчёты](bins/kronika-report/README.ru.md)
