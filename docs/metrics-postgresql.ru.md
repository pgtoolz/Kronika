# Справочник метрик PostgreSQL

[English version](metrics-postgresql.md) · [Оглавление справочника](features.ru.md)

<a id="источники-identity-и-обозначения"></a>

## Источники, ключи строк и обозначения

Сборщик читает общие метрики сервера через первое подключение из `KRONIKA_PG_DSNS`. Представления, относящиеся к отдельной базе, он читает в каждой базе, к которой может подключиться. Для запросов и планов выбирается одна обнаруженная установка каждого расширения. [Справочник форматов](type-registry/postgresql-metrics.ru.md) перечисляет поддерживаемые версии PostgreSQL и расширений, идентификаторы типов и область сбора. [Код описания полей](../crates/kronika-registry/src/codec) задаёт состав каждой записываемой строки.

Снимок — строки одного чтения источника. Для двух снимков со временем `t₀ < t₁` обозначим изменение счётчика как `Δx = x₁ − x₀`, длину интервала как `d = (t₁ − t₀)/10⁶` секунд, скорость изменения как `r(x) = Δx/d`. `x₀` и `x₁` — значения одного и того же счётчика в моменты `t₀` и `t₁`. Эти метки времени выражены в микросекундах Unix. Например, `r(calls)` отвечает на вопрос, сколько выполнений запроса в секунду пришлось на этот интервал. Отдельное `x` означает значение в выбранном снимке: текущую величину или накопленный счётчик.

`B` — размер блока PostgreSQL в байтах: положительное целое `pg_settings.block_size` из снимка у выбранного момента (курсора). Столбцы чтения и обращения к буферам переводят блоки в байты по формуле `B × r(blocks)`; **Buffer bytes/call** — по формуле `B × blocks_per_call`, где `blocks_per_call` — число обращений к блокам на одно выполнение запроса. Без записанного `B` эти значения в байтах недоступны.

В **Statements** и **Plans** разности вычисляются между строками одного формата с одинаковым ключом из таблицы ниже. Интервал должен быть положительным, а каждый используемый счётчик — неубывающим. Отсутствующее значение или уменьшение счётчика делает зависимое вычисление недоступным для этой пары строк; следующие пары проверяются отдельно. У отношений знаменатель должен быть положительным. Записанные `min`, `max`, `mean`, `stddev`, метки времени и возрасты используются как готовые значения, даже если соседний накопленный счётчик уменьшился. Для **Tables**, **Indexes** и **Overview** правила отсутствующих значений описаны в соответствующих разделах. Источники: [интервальные вычисления](../bins/kronika-web/ui/src/postgres-metrics.ts), [перевод в байты и выбор столбцов](../bins/kronika-web/ui/src/postgres-view.tsx).

| Поток данных | Поля, которые определяют один объект |
|---|---|
| Activity, Locks | Числовой `pid` |
| Database | `datid` |
| Statement | `userid, dbid, queryid`; форматы с `toplevel` добавляют его в ключ |
| Plan | `userid, dbid, queryid, planid` в одном формате расширения |
| Table / Index | `datid, relid` / `datid, indexrelid` |
| I/O | `backend_type, object, context` |
| Settings | `datid, usesysid, name` |

## Activity и Locks

PID — идентификатор процесса в ОС; OID — идентификатор объекта в PostgreSQL. XID обозначает идентификатор транзакции, MXID — идентификатор multixact, который объединяет несколько транзакций, блокирующих одну строку.

**Activity** показывает состояние серверных процессов PostgreSQL. По умолчанию скрыты строки с `state = idle`, а также служебные процессы: непустой `backend_type`, отличный от `client backend`. Отсутствующий или пустой `backend_type` не скрывает строку. Переключатели **Idle** и **System** включают эти строки; явно открытая строка остаётся видимой. Сначала идут запросы с наибольшей длительностью. Если у обеих сравниваемых строк длительность запроса недоступна, используется длительность транзакции. Источники: [столбцы и фильтры Activity](../bins/kronika-web/ui/src/postgres-view.tsx), [вычисление длительностей](../bins/kronika-web/ui/src/postgres-activity.ts).

| Показатель / поле | Определение и единица |
|---|---|
| Query duration / `query_duration_ms` | `(t − query_start)/1000` мс, только при `state = active` |
| Transaction duration / `transaction_duration_ms` | `(t − xact_start)/1000` мс при любом состоянии с известным началом транзакции |
| State duration / `state_duration_ms` | `(t − state_change)/1000` мс, скрыта при `state = idle` |
| Backend age / `backend_age_ms` | `(t − backend_start)/1000` мс |
| PID, leader PID | PID серверного процесса в ОС и записанный PID ведущего процесса параллельного запроса |
| Database, role, application, client | `datname`, `usename`, `application_name`, `client_addr` из `pg_stat_activity` |
| State, wait event type, wait event | Записанные значения сервера; ожидание может присутствовать при `state = active` |
| Query, query ID | Записанный SQL и `query_id`; при сборе текст запроса ограничен 65 536 символами |
| XID / xmin age | Серверные `age(backend_xid)` / `age(backend_xmin)`, в идентификаторах транзакций |

В формулах длительности `t` — время записанной строки, а время начала взято из указанного поля PostgreSQL. Обе метки выражены в микросекундах Unix; деление на 1000 даёт миллисекунды. Отсутствующая, неположительная или будущая метка начала даёт `null`. В неактивной строке SQL содержит последний записанный запрос, но длительность активного запроса отсутствует.

**Locks** показывает процессы, связанные ожиданием блокировки: кто ждёт и кто мешает ему продолжить работу. На каждый участвующий процесс записывается одна строка. Для ожидающего процесса сборщик выбирает по фиксированному правилу одну строку `pg_locks` с `granted = false` и сохраняет отсортированный список различных `pg_blocking_pids(pid)`. Собственный PID сборщика и процессы с `application_name`, равным имени его подключения, исключены. Значение `blocked_by = 0` означает подготовленную транзакцию. Источник: [SQL сбора Locks](../crates/kronika-source-pg/src/locks.rs).

| Поле или действие в Locks | Значение |
|---|---|
| PID tree | Блокирующий процесс расположен перед ожидающим; дополнительные блокирующие процессы показаны как `+PID`; строки каждой независимой цепочки расположены вместе |
| Prepared transaction | У `0` есть подпись, но не создаётся строка процесса или ребро дерева |
| Количество строк | Число показанных процессов, включая блокирующие; это не число удерживаемых блокировок |
| `lock_locktype`, `lock_mode` | Тип и запрошенный режим выбранной ожидаемой блокировки |
| `lock_target` | Найденное имя таблицы или индекса, идентификатор обычной или виртуальной транзакции, идентификаторы объекта или тип блокировки |
| `lock_database`, `lock_relation`, `lock_page`, `lock_tuple` | OID базы и объекта, необязательные номера страницы и строки |
| `lock_classid`, `lock_objid`, `lock_objsubid` | Идентификаторы объекта каталога для блокировок объектов |
| `waitstart` | Серверное время начала ожидания блокировки, PostgreSQL 14+; показана абсолютная метка времени |
| Backend context | База, роль, приложение, клиент, состояние, ожидание и запрос из того же запроса сбора Locks |
| Search | Вместе с найденными строками показаны блокирующие их предки и дополнительные блокирующие процессы |

Имя таблицы или индекса определяется только в базе подключения сборщика. У процесса в корне цепочки могут отсутствовать поля ожидаемой блокировки. Панель **Inspector** в **Locks** показывает сведения о процессе и `blocked_by`; ячейки PID и объекта блокировки не содержат переходов к связанным объектам. Источники: [построение дерева и поиск](../bins/kronika-web/ui/src/postgres-locks.ts), [отображение Locks](../bins/kronika-web/ui/src/postgres-view.tsx).

## Overview и Databases

**Overview** сопоставляет показатели всего загруженного часа с выбранным моментом: слева показан итог за час, справа — значение графика у курсора. Скорости разных объектов складываются, только если совпадает время второго снимка в их расчётных парах. Итог за час — сумма доступных приращений счётчика. Недоступная пара для одного поля пропускается; если доступных пар нет, итог равен `null`. Сумма и максимум текущих значений также пропускают `null`; если весь снимок состоит из `null`, результат остаётся `null`. Источники: [определения Overview](../bins/kronika-web/ui/src/postgres-overview.tsx), [расчёт итогов](../bins/kronika-web/ui/src/postgres-vitals.ts).

В таблице `Σ` означает сумму по базам, если явно не указан другой набор объектов. **Итог** — сумма приращений за час; **пик** — максимум точек графика; **последнее** — последняя точка графика, отличная от `null`.

| Строка Overview | График / значение у курсора | Левое число |
|---|---|---|
| Client backends | Число `backend_type = client backend` в Activity; отсутствующий тип процесса заменяется на `client backend` | Пик; `/ max_connections`, если записан |
| Active vs waiting | Активные клиентские процессы без / с `wait_event_type`; ведомые процессы параллельного запроса исключены | Пик числа выполняющихся процессов |
| Idle in transaction | Число состояний, начинающихся с `idle in transaction`, во всей Activity | Пик |
| Oldest transaction | Максимум `(t − xact_start)/10⁶` с среди клиентских процессов без ведомых процессов параллельного запроса; минимум 0 | Пик длительности |
| Oldest xmin age | Максимум записанного `backend_xmin_age` во всей Activity | Пик, идентификаторы транзакций |
| Prepared transactions | Сумма `prepared_count`; возраст — максимум `max_age_us` | Пик числа и возраста |
| Transactions | `Σ[r(xact_commit)+r(xact_rollback)]`; второй ряд `Σr(xact_rollback)` | Итоговое число транзакций и `100 × total rollbacks / total transactions`, где оба total — число откатов и всех транзакций за час |
| Tuples read | `Σr(tup_returned)`; второй ряд `Σr(tup_fetched)` | Итоговое число возвращённых строк |
| Tuples written | `Σ[r(tup_inserted)+r(tup_updated)+r(tup_deleted)]` | Итоговое число изменённых строк |
| Buffer hit share | `100Σr(blks_hit)/Σ[r(blks_hit)+r(blks_read)]`, % | То же отношение по приращениям за час |
| Block I/O time | `Σ[r(blk_read_time)+r(blk_write_time)]`, мс/с | Итог, мс |
| Temp bytes | `Σr(temp_bytes)`, байт/с | Итог, байты |
| Deadlocks / Checksum failures | `Σr(deadlocks)` / `Σr(checksum_failures)`, событий/с | Итоговое число событий |
| Abnormal session ends | `Σ[r(sessions_fatal)+r(sessions_killed)]`, сеансов/с | Итоговое число сеансов |
| WAL generated | `r(pg_stat_wal.wal_bytes)`, байт/с | Итог, байты |
| Checkpoints | Число плановых / запрошенных контрольных точек в секунду | Итоговое число всех и запрошенных контрольных точек |
| Checkpoint buffer writes | `r(buffers_checkpoint)`; при отсутствии, PostgreSQL 17+ — `r(pg_stat_checkpointer.buffers_written)`; второй ряд `r(buffers_backend)`, если доступен, блоков/с | Итоговое число блоков контрольных точек |
| WAL archiver | `r(archived_count)` / `r(failed_count)`, событий/с | Оба итога |
| WAL buffers full | `r(wal_buffers_full)`, событий/с | Итоговое число событий |
| pg_wal size | Записанное `wal_files_bytes`, байты | Последнее |
| Buffer evictions / Buffer reuses | Сумма `r(evictions)` / `r(reuses)` по ключам строк `pg_stat_io`, операций/с | Итоговое число операций |
| Relation extends / Fsyncs | Сумма `r(extends)` / `r(fsyncs)` по ключам строк I/O, операций/с | Итоговое число операций |
| Vacuum reads | Сумма `r(reads)` при `pg_stat_io.context = vacuum`, операций/с | Итоговое число операций |
| Transaction ID age | Максимум по базам `frozen_xid_age`, идентификаторы транзакций | Последнее; процент от записанного `autovacuum_freeze_max_age` |
| Multixact age | Максимум по базам `min_mxid_age`, идентификаторы multixact | Последнее |
| Autovacuum workers | Число строк Vacuum progress с `is_autovacuum = true` | Пик; `/ autovacuum_max_workers`, если записан |

Для числа плановых и запрошенных контрольных точек сначала используются `pg_stat_checkpointer.num_timed/num_requested`, а при их отсутствии — `pg_stat_bgwriter.checkpoints_timed/checkpoints_req`. Снимки подготовленных транзакций и хода VACUUM относятся к открытому интервалу между соседними метками снимков баз. Пустой интервал даёт нулевое число подготовленных транзакций и работников autovacuum; возраст подготовленных транзакций остаётся `null`.

Пунктирные границы **Overview** берутся из последних записанных настроек за загруженный час. **Passport** — карточка сведений о сервере — использует настройки у курсора, а если курсор стоит раньше первой записи, берёт первую запись часа. Графики **Overview** текущего часа обновляются не чаще раза в минуту. [Код сводки Activity](../crates/kronika-query/src/hour/lanes.rs) задаёт отбор клиентских процессов и исключение участников параллельного запроса.

В **Databases** текущее число подключений — записанное `numbackends`. Счётчики транзакций, сеансов, строк, буферов, временных файлов, конфликтов и взаимоблокировок показываются как скорости между соседними снимками. `blk_read_time` и `blk_write_time` выражены в мс/с, `temp_bytes` — в байтах/с. `frozen_xid_age = age(pg_database.datfrozenxid)` используется как записанный возраст, в идентификаторах транзакций. Число блоков `blks_read` и `blks_hit` переводится в байты/с через `B`.

`tup_returned` считает строки, возвращённые сканированиями, а `tup_fetched` — живые строки, извлечённые при индексных сканированиях. `conflicts` считает запросы, отменённые из-за конфликтов с восстановлением. Строка с `datid = 0` содержит статистику общих объектов; она не входит в отображаемое число баз. Источники: [SQL и поля баз](../crates/kronika-source-pg/src/database.rs), [столбцы таблицы](../bins/kronika-web/ui/src/postgres-view.tsx).

## Statements и Plans

### Интервальные метрики

**Statements** показывает затраты на нормализованные запросы, **Plans** — на записанные планы их выполнения. Переключатель **Lens** выбирает набор столбцов: у Statements это **Execution**, **Per call**, **I/O**, **Resources**, **Stability**; у Plans — **Execution**, **Timing**, **I/O**, **Identifiers**. Названию **Execution** в ссылках соответствует значение `load`. Общие столбцы определяют базу, роль и идентификаторы запроса или плана; первый столбец содержит SQL или краткое описание плана. Источники: [наборы столбцов и вычисления](../bins/kronika-web/ui/src/postgres-metrics.ts), [отображение столбцов](../bins/kronika-web/ui/src/postgres-view.tsx).

Показатель **Planning time/s** показывает затраты на построение планов, **Plan time, %** — их долю в сумме времени планирования и выполнения. **CV** — отношение стандартного отклонения времени выполнения к среднему: чем оно больше, тем сильнее различаются длительности выполнений.

В формулах `E` — накопленное время выполнения в миллисекундах: `total_exec_time` для форматов `pg_stat_statements` 1.8+ и `total_time` для 1.5–1.7 и всех форматов планов. `P = total_plan_time` — накопленное время планирования в миллисекундах; `C = calls` — число выполнений. Счётчики блоков: `H = shared_blks_hit`, `R = shared_blks_read`, `LH = local_blks_hit`, `LR = local_blks_read`. Попадание означает обращение к уже находящемуся в буфере блоку; чтение — загрузку блока в буфер.

| Показатель / вычисляемое поле | Формула / записанное значение | Единица; набор столбцов |
|---|---|---|
| Calls/s | `r(C)` | выполнений/с; Execution, Per call, Resources, Stability, Timing, Identifiers |
| Exec time/s | `r(E)` | мс/с; Execution, Resources; время одновременно выполняющихся запросов складывается, поэтому значение может превышать 1000 |
| Mean/call | `ΔE/ΔC` | мс/выполнение; Execution, Per call |
| Rows/s | `r(rows)` | возвращённых или изменённых строк/с; Execution |
| Rows/call | `Δrows/ΔC` | строк/выполнение; Per call |
| Buffer bytes/call | `B × [r(H)+r(R)+r(LH)+r(LR)]/r(C)` | байт/выполнение; Per call, I/O |
| Cache hit ratio / `hit_pct` | `100r(H)/[r(H)+r(R)]` | %; I/O; только общие буферы PostgreSQL |
| Shared/local buffer hits, reads, dirtied, written | `B × r(shared_blks_hit/read/dirtied/written)` и соответствующие `local_blks_*` | байт/с; I/O и записанные поля |
| Temp reads / writes | `B × r(temp_blks_read/written)` | байт/с; I/O, Resources |
| WAL bytes | `r(wal_bytes)` | байт/с; Resources |
| WAL/call | `Δwal_bytes/ΔC` | байт/выполнение; Resources |
| Planning time/s | `r(P)` | мс/с; Resources |
| Plan time, % | `100r(P)/[r(P)+r(E)]` | %; Resources |
| WAL records / FPI / buffers full | `r(wal_records/wal_fpi/wal_buffers_full)` | записей WAL / полных образов страниц / событий заполнения буфера в секунду; записанные поля |
| Plans | `r(plans)` | планирований/с; записанное поле запроса |
| Shared/local/temp read/write time | `r(physical timing field)` | мс/с; записанные поля |
| Mean / Min / Max / Stddev | Записанные минимум, максимум, среднее и стандартное отклонение времени выполнения за период накопления статистики расширения | мс; Stability, Timing |
| CV | Записанное `stddev / mean` | Безразмерное; Stability; null при неположительном среднем |
| Calls | Записанное накопленное целое `calls` | выполнений; Plans → Identifiers |
| Slow log calls | `r(slow_log_calls)` в формате vadv | выполнений/с; записанное поле плана |
| First call / Last call / Stats since | Записанные `first_call`, `last_call`, `stats_since`, если формат их содержит | Абсолютное время |

**Exec time/s** показывает суммарное время выполнения за секунду наблюдения, а **Mean/call** — среднее время одного выполнения на выбранном интервале. **Mean** берётся непосредственно из статистики расширения и относится к её периоду накопления. В таблице **Buffer bytes/call** складывает доступные конечные скорости по блокам; нужна хотя бы одна такая скорость. При построении истории этого показателя нужны все запрошенные скорости по блокам. Для остальных отношений необходимы все перечисленные в формуле величины. Перевод блоков в байты не меняет их процентные отношения.

| Формат | Поля времени выполнения / планирования | Поля времени работы с блоками |
|---|---|---|
| Statements 1.5–1.7 | `total_time`; время планирования отсутствует | `blk_read_time`, `blk_write_time` измеряют работу с общими буферами |
| Statements 1.8–1.9 | `total_exec_time`, `total_plan_time` | Те же имена полей общих буферов |
| Statements 1.10 | Те же | Поля общих буферов плюс `temp_blk_read_time`, `temp_blk_write_time` |
| Statements 1.11+ | Те же | `shared_blk_*_time`, `local_blk_*_time`, `temp_blk_*_time` |
| OSSC / Datasentinel plans | `total_time`; время планирования отсутствует | Раздельные поля для общих, локальных и временных блоков |
| vadv plans | `total_time`, `total_plan_time` | `blk_read_time`, `blk_write_time` измеряют работу с общими буферами |

В форматах запросов 1.5–1.7 готовые показатели времени называются `min_time/max_time/mean_time/stddev_time`, в более новых — `min_exec_time/max_exec_time/mean_exec_time/stddev_exec_time`. Планы используют первые имена. Выбранный **Lens** и панель **Inspector** показывают часть полей из таблиц выше; определения остальных полей совпадают с источником. Если формат не содержит поля, оно скрыто или недоступно. Точные поля: [описание запросов](../crates/kronika-registry/src/codec/pg_stat_statements.rs), [описание планов](../crates/kronika-registry/src/codec/pg_store_plans.rs).

<a id="plan-identity-текст-и-related-controls"></a>

### Ключ плана, текст и переходы

В вариантах OSSC и Datasentinel поле `queryid` содержит идентификатор запроса. В варианте vadv `queryid` — внутренний идентификатор расширения и часть ключа из четырёх полей; `queryid_stat_statements` хранит последнюю записанную связь с запросом и может меняться отдельно. При нулевом значении этой связи переход к запросу отсутствует. Datasentinel дополнительно записывает `relids` — список OID таблиц и индексов — и `cmd_type` — тип SQL-команды. Текст плана получает функция расширения, возвращающая читаемое представление; сохраняются первые 65 536 символов.

Нажатие **Query ID** в Statements открывает Plans с условиями по базе, роли и идентификатору запроса. Обратный переход из Plans использует те же условия и записанную связь плана с запросом. **Plan ID** отбирает строки по идентификатору плана. Переход из Activity в Statements требует ненулевых `datid` и `query_id`, а также имени базы; отбор выполняется по базе и идентификатору запроса. При загрузке SQL в Inspector плана поиск использует только идентификатор запроса. Источники: [условия переходов](../bins/kronika-web/ui/src/statement-navigation.ts), [поиск SQL плана](../bins/kronika-web/ui/src/plan-query.ts), [функции сбора](../crates/kronika-source-pg/src/store_plans.rs).

Переключатель **Kronika queries** включает запросы самого сборщика. По умолчанию используется набор `workload`, который их исключает. Непустая строка поиска, отбор по конкретному запросу или явное открытие запроса сборщика включает набор `all` и блокирует переключатель. Тот же выбор действует для сводки и графиков активности запросов. Источники: [выбор набора запросов](../bins/kronika-web/ui/src/postgres-view.tsx), [распознавание запросов сборщика](../crates/kronika-query/src/statement_scope.rs).

## Tables и Indexes

<a id="группировка-storage-и-null"></a>

### Группировка, размеры и отсутствующие значения

Переход **Databases → Schemas → Objects** сначала выбирает базу по OID, затем отбирает объекты по базе и схеме. **Tablespaces → Objects** выбирает фактическое табличное пространство по OID в пределах сервера. Индекс может находиться в собственном табличном пространстве. При `reltablespace = 0` используется `dattablespace` базы. Родитель секционированной таблицы без собственных файлов не имеет такого размещения и не входит в группы табличных пространств. Переходы **Table indexes** и **Table** используют точную пару `datid, relid`. **Index definition** загружает записанное поле `indexdef` выбранного объекта. Источники: [сбор таблиц](../crates/kronika-source-pg/src/user_tables.rs), [сбор индексов](../crates/kronika-source-pg/src/user_indexes.rs), [переходы](../bins/kronika-web/ui/src/postgres-relations.ts).

Скорость для группы — сумма скоростей её объектов: `R(x) = Σᵢ Δxᵢ/dᵢ`, где `i` — объект, `Δxᵢ` — приращение его счётчика, `dᵢ` — собственный интервал между снимками в секундах. Сумма текущих значений — `G(x) = Σᵢ xᵢ`. В отношениях сначала складываются числители и знаменатели, затем выполняется деление. Возрасты XID и MXID обобщаются максимумом. Для меток времени группа хранит самое раннее и самое позднее значения, а также число записанных `null` — столбцы **Never…**. Таблица без TOAST не добавляет ни его метку времени, ни запись в счётчик Never для TOAST.

Если формат не содержит поля или пара снимков объекта непригодна для расчёта, соответствующий показатель группы равен `null`. Для счётчиков индексов и TOAST у таблицы два явно записанных `null` означают, что этого компонента нет; такая пара не добавляет значение. Текущие значения TOAST с `null` также не влияют на сумму, если `toast_bytes` равен `null`. Составные показатели скорости, буферов и размера допускают эти исключения для отсутствующих компонентов. Показатель отдельного компонента без применимых значений остаётся `null`. Оценка числа строк `reltuples = -1` считается недоступной. Источник: [расчёт показателей объектов, `Aggregate::metric`, `counter_input`, `gauge_input`](../crates/kronika-query/src/hour/relation.rs).

| Поле размера | Точный источник; байты |
|---|---|
| Table data / `main_fork_bytes` | `pg_relation_size(table_oid)`: основной файл таблицы (main fork) |
| TOAST / `toast_bytes` | `pg_total_relation_size(reltoastrelid)`: таблица TOAST со вспомогательными файлами и индексами; null при отсутствии TOAST |
| Table + TOAST / `displayed_storage_bytes` | Основной файл таблицы + TOAST; пользовательские индексы и файлы FSM/VM основной таблицы не входят |
| Index data / `main_fork_bytes` | `pg_relation_size(index_oid)`: основной файл индекса |

<a id="table-lenses"></a>

### Наборы столбцов Tables

HOT — обновление строки с сохранением её на той же странице без новых записей в обычных индексах. TOAST — отдельное хранение больших значений; его таблица и индекс учитываются отдельно от основной таблицы. Доля попаданий в буферы сравнивает обращения к уже загруженным блокам с суммой таких обращений и чтений.

**Access** показывает способы чтения таблицы, **Changes** — изменения строк, **Maintenance** — выполненные VACUUM и ANALYZE, **Size and buffers** — размеры и обращения к буферам, **Freeze** — возраст идентификаторов транзакций и последние обслуживания. Для одного объекта `R = r`, `G(x) = x`; для группы действуют определения выше. `D = R(n_tup_ins)+R(n_tup_upd)+R(n_tup_del)` — суммарное число вставленных, обновлённых и удалённых строк в секунду.

| Набор столбцов: показатель / поле | Формула, единица и значение |
|---|---|
| Access: Tuples scanned | `R(seq_tup_read)+R(idx_tup_fetch)`, строк/с |
| Seq scans / Index scans | `R(seq_scan)` / `R(idx_scan)`, сканирований/с |
| Seq scans, % | `100R(seq_scan)/[R(seq_scan)+R(idx_scan)]` |
| Tuples per seq/index scan | `R(seq_tup_read)/R(seq_scan)` / `R(idx_tup_fetch)/R(idx_scan)` |
| Last seq/index scan | Записанные метки времени, PostgreSQL 16+; для групп — самая ранняя и поздняя метки, а также число null (Never) |
| Changes: DML total | `D`, вставленных + обновлённых + удалённых строк/с |
| Inserts / Updates / Deletes, % | `100R(n_tup_ins)/D`, `100R(n_tup_upd)/D`, `100R(n_tup_del)/D` |
| HOT updates, % | `100R(n_tup_hot_upd)/R(n_tup_upd)` |
| New-page updates, % | `100R(n_tup_newpage_upd)/R(n_tup_upd)`, PostgreSQL 16+ |
| Dead tuples, % | `100G(n_dead_tup)/[G(n_live_tup)+G(n_dead_tup)]`, оценки выбранного снимка |
| Modified since analyze | `G(n_mod_since_analyze)`, оценка числа изменённых строк после последнего ANALYZE |
| Inserted since vacuum | `G(n_ins_since_vacuum)`, оценка числа вставленных строк после последнего VACUUM, PostgreSQL 13+ |
| Maintenance: Manual vacuums / Autovacuums / Manual analyzes / Autoanalyzes | `R(vacuum_count/autovacuum_count/analyze_count/autoanalyze_count)`, операций/с |
| Mean manual vacuum / autovacuum / manual analyze / autoanalyze | `R(total_vacuum_time)/R(vacuum_count)` и соответствующие пары времени и числа операций, мс/операцию; PostgreSQL 18+ |
| Last manual vacuum / autovacuum / manual analyze / autoanalyze / TOAST autovacuum | Записанные метки времени; для групп — самая ранняя и поздняя метки, а также число null (Never) |
| Size and buffers: Table + TOAST / Table data | Соответствующие суммы записанных размеров, байты |
| TOAST, % | `100G(toast_bytes)/[G(main_fork_bytes)+G(toast_bytes)]` |
| Estimated rows | `G(reltuples)`, `pg_class.reltuples`, приведённый к целому при сборе |
| TOAST dead tuples, % | `100G(toast_n_dead_tup)/[G(toast_n_live_tup)+G(toast_n_dead_tup)]` |
| Heap / Index / TOAST / TOAST-index buffer hit share | Для префикса полей `q = heap, idx, toast, tidx` (основная таблица, её индексы, TOAST, индекс TOAST): `100R(q_blks_hit)/[R(q_blks_hit)+R(q_blks_read)]` |
| Buffer hit share | Та же формула с суммами по четырём префиксам |
| Buffer read/hit bytes | `B × R(q_blks_read/hit)`, байт/с; для четырёх компонентов хранения, перечисленных выше |
| Freeze: XID age / MXID age | Максимум `age(pg_class.relfrozenxid)` / `mxid_age(pg_class.relminmxid)`; идентификаторы транзакций / multixact |
| Freeze: Inserted since vacuum; last vacuum/autovacuum | Записанные значения и метки времени, определённые выше |

<a id="index-lenses"></a>

### Наборы столбцов Indexes

**Usage** показывает обращения к индексу и полученные строки, **Size and buffers** — размер и работу с буферами, **State** — свойства индекса и его готовность к использованию. Признак **valid** означает пригодность для запросов, **ready** — обновление индекса при изменении строк; **unique**, **primary**, **exclusion** обозначают уникальность, первичный ключ и ограничение исключения.

| Набор столбцов: показатель / поле | Формула, единица и значение |
|---|---|
| Usage: Index scans | `R(idx_scan)`, сканирований/с |
| Index tuples read / Tuples fetched | `R(idx_tup_read)` / `R(idx_tup_fetch)`, записей индекса / живых строк таблицы в секунду |
| Tuples per scan / Fetches per scan | `R(idx_tup_read)/R(idx_scan)` / `R(idx_tup_fetch)/R(idx_scan)` |
| Last index scan | Записанная метка времени, PostgreSQL 16+; для групп — самая ранняя и поздняя метки и число null (Never) |
| Low activity | Применяет `no_scans = true`: доступное `Δidx_scan = 0` в текущей паре снимков |
| No scans / No-scan count / Known-scan count | Логическое значение для объекта; число объектов группы с `Δidx_scan = 0`; число объектов группы с `Δidx_scan > 0` |
| Size and buffers: Index data | `G(main_fork_bytes)`, байты |
| Buffer hit share | `100R(idx_blks_hit)/[R(idx_blks_hit)+R(idx_blks_read)]` |
| State: valid / ready / unique / primary / exclusion | Логические значения `pg_index.indisvalid/indisready/indisunique/indisprimary/indisexclusion` |
| State group counts | Число объектов с ложным valid/ready или истинным unique/primary/exclusion |
| State order / `state_severity` | `2` invalid (непригоден); `1` valid (пригоден), но не ready (не поддерживается при изменении строк); `0` valid и ready; максимум группы |
| Access method | `pg_am.amname`; Index definition — записанный `pg_get_indexdef(index_oid)` |

**Low activity** отбирает индексы без сканирований на текущем интервале. Нулевая скорость не означает, что накопленный счётчик равен нулю или что индекс не использовался раньше. Источники обеих таблиц: [определения показателей и история](../bins/kronika-web/ui/src/postgres-relations.ts), [расчёт групп](../crates/kronika-query/src/hour/relation.rs).

## Vacuum progress

**Vacuum progress** показывает ход записанных запусков VACUUM: текущую фазу, обработанные блоки и затраты процесса. Источник — `pg_stat_progress_vacuum`, соединённое с Activity для признака `is_autovacuum` и с каталогами базы подключения для имён объектов. Ключ строки — `pid, datid, relid`; отдельные форматы соответствуют PostgreSQL 10–16, 17 и 18. Источники: [сбор](../crates/kronika-source-pg/src/progress_vacuum.rs), [выделение запусков и расчёты](../bins/kronika-web/ui/src/postgres-vacuum.ts), [отображение](../bins/kronika-web/ui/src/postgres-view.tsx).

| Поле / показатель | Определение |
|---|---|
| Kind | `is_autovacuum = (backend_type = autovacuum worker)`; false, если соединение с Activity не нашло строку |
| Phase | Записанное название фазы |
| Scan progress | `clamp(100 × heap_blks_scanned / heap_blks_total, 0, 100)`; null при неположительном или отсутствующем общем числе блоков |
| Heap scanned / total | `B × heap_blks_scanned` / `B × heap_blks_total`, байты; total — число блоков таблицы в начале сканирования |
| Heap vacuumed | `B × heap_blks_vacuumed`, байты |
| Index cycles | Записанный `index_vacuum_count`; значения >1 получают подпись повторного прохода |
| Index progress | `indexes_processed / indexes_total`, PostgreSQL 17+, показывается только в фазах vacuuming/cleaning-up indexes |
| Dead tuple capacity/use, PostgreSQL 10–16 | `max_dead_tuples`, `num_dead_tuples`, число строк |
| TID store, PostgreSQL 17+ | `max_dead_tuple_bytes`, `dead_tuple_bytes` — ёмкость и занятый размер хранилища адресов мёртвых строк, в байтах; `num_dead_item_ids` — число адресов |
| Cost delay, PostgreSQL 18 | Накопленный `delay_time`, мс; `+Δdelay_time` по двум последним снимкам запуска при неубывающем счётчике |
| In phase | Число последних снимков с одинаковой фазой и номером прохода по индексам; время между первым и последним снимком этой серии |
| No movement | Не менее 3 последних снимков с неизменным счётчиком из таблицы ниже в одной фазе и проходе; показаны число снимков и время между ними |

Запуск VACUUM определяется форматом записи, PID, OID базы и OID таблицы. При том же ключе новый запуск начинается, если уменьшается `index_vacuum_count`, `heap_blks_scanned` или `heap_blks_vacuumed`, либо разрыв между снимками превышает `2.5 × recorded postgresql_interval_seconds`. Здесь `recorded postgresql_interval_seconds` — записанный интервал сбора PostgreSQL в секундах. Если положительный интервал не записан, используются только ключ и уменьшение счётчиков. Строка показывает последний снимок запуска. Отметка **At cursor** означает, что его время совпадает с последним временем снимка Vacuum progress не позже курсора. Для остальных запусков показывается их собственное последнее записанное время.

| Phase | Фиксированная отметка фазы | Величина для No movement |
|---|---|---|
| `initializing`, `performing final cleanup` | Обычная (Ordinary) | Нет |
| `scanning heap` | Обычная (Ordinary) | `heap_blks_scanned` |
| `vacuuming heap` | Нагрузка (Heavy) | `heap_blks_vacuumed` |
| `vacuuming indexes`, `cleaning up indexes` | Нагрузка (Heavy) | `indexes_processed`, PostgreSQL 17+ |
| `truncating heap` | Опасная (Dangerous) | Повторные снимки этой фазы; фаза устанавливается до условной попытки захватить исключительную блокировку |

Раздел **Process load** показывает затраты процесса за время наблюдаемого запуска. Для его PID выбираются снимки ОС не позже первой и последней меток запуска. `H` — записанное число тактов часов ОС в секунду. Тогда время CPU в миллисекундах равно `1000Δ(utime+stime)/H`, где `utime` и `stime` — процессорные такты в пользовательском и системном режиме. Доля CPU равна `min(100,100 × CPU seconds / OS sample elapsed seconds)`: процессорное время делится на время между выбранными снимками ОС, обе величины — в секундах. Время ожидания блочного ввода-вывода равно `1000Δblkdelay_ticks/H` мс.

Прочитанные и записанные байты, а также число страничных ошибок с чтением с диска равны `Δread_bytes`, `Δwrite_bytes`, `Δmajflt`. Доля прочитанных байтов — `min(100,100Δread_bytes/(B × final heap_blks_scanned))`, где `final heap_blks_scanned` — число просмотренных блоков таблицы в последнем снимке запуска. Эти разности включают всю работу PID между выбранными снимками ОС. Отсутствующий размер блока или частота часов делает недоступными только зависящие от него вычисления.

<a id="summary-strips-и-цветовые-отметки"></a>

## Сводки и цветовые отметки

`count(...)` ниже означает число подходящих объектов, `usable` — наличие пригодного значения по правилам соответствующего раздела, `AND` — одновременное выполнение обоих условий.

Сводка над таблицей обобщает все объекты в последнем снимке не позже курсора. Страница таблицы и текстовый поиск не ограничивают этот набор. В Statements и Plans учитывается выбранный набор запросов. Для каждого отношения берутся только объекты с пригодной парой необходимых значений; числитель и знаменатель накапливаются по одному набору объектов. Здесь `ΣΔ` — сумма таких приращений в выбранном снимке сводки, а не за весь час. Источники: [выбор данных сводки](../crates/kronika-query/src/hour/postgres_summary.rs), [реализация формул](../crates/kronika-query/src/hour/postgres_summary/facts.rs).

| Сводка | Формула |
|---|---|
| Active statements / Used plans | Число доступных `Δcalls > 0`; процент от объектов с доступным `Δcalls` |
| Execution per call | `ΣΔE / ΣΔcalls` |
| WAL per call | `ΣΔwal_bytes / ΣΔcalls` |
| Executions per used plan | `ΣΔcalls / count(Δcalls > 0)` |
| Reads outside buffers | `100ΣΔread / ΣΔ(read+hit)`; Statements/Plans: общие и локальные буферы; Databases: `blks_*`; Tables: основная таблица, её индексы, TOAST и индекс TOAST; Indexes: только индекс |
| Rollbacks / Temp per transaction | `100ΣΔxact_rollback / ΣΔ(xact_commit+xact_rollback)`; `ΣΔtemp_bytes / ΣΔ(xact_commit+xact_rollback)` |
| Scan methods | `100ΣΔseq_scan / ΣΔ(seq_scan+idx_scan)`; доля индексных сканирований дополняет до 100% |
| HOT updates / Dead rows | `100ΣΔn_tup_hot_upd/ΣΔn_tup_upd`; `100Σn_dead_tup/Σ(n_live_tup+n_dead_tup)` |
| Vacuumed tables | `100 × count[Δ(vacuum_count+autovacuum_count)>0] / count[usable Δ(vacuum_count+autovacuum_count)]` |
| Storage | `100Σtoast_bytes/Σ(main_fork_bytes+toast_bytes)`; доля основной таблицы дополняет до 100% |
| XID boundary | `100 × count(xid_age ≥ 1,600,000,000) / count(usable xid_age)` |
| Scanned indexes / Without scans | Процент положительных / нулевых среди доступных `Δidx_scan` |
| Usable indexes | `100 × count(indisvalid AND indisready) / count(objects with both flags)` |

Цвет значения определяется фиксированными границами из [value-tone.ts](../bins/kronika-web/ui/src/value-tone.ts). Для длительности запроса и транзакции цвет применяется только к активным клиентским процессам. Нулевые скорости сначала получают цвет неактивного значения; приведённые ниже границы к ним не применяются.

| Значение | Good | Warning | Critical |
|---|---|---|---|
| Query duration | — | 1000–<5000 мс | ≥5000 мс |
| Transaction duration | — | 5000–<60000 мс | ≥60000 мс |
| Mean/call, Mean | — | — | ≥5000 мс |
| Cache hit ratio | ≥99% | 90–<99% | <90% |
| CV | <1 | 1–<3 | ≥3 |
| Plan time | <50% | 50–<80% | ≥80% |
| State | — | `idle in transaction` | `idle in transaction (aborted)` |
| Wait event type | — | Любое непустое значение | — |

<a id="settings-reset-timestamps-и-wal-storage"></a>

## Настройки, время сброса статистики и размер WAL

`pg_settings` содержит действующие настройки подключения, которым сборщик читает метрики; ключ включает базу и роль подключения. Сборщик записывает настройки после первого успешного чтения, при изменении и в каждом новом сегменте. Поля `primary_conninfo` и `ssl_passphrase_command` исключены. В **Passport** показаны версия сервера, предел подключений, размер общих буферов, максимальный размер WAL, интервал контрольных точек, настройки autovacuum и измерения времени ввода-вывода. Список изменений сравнивает последовательные записанные строки `setting` по имени настройки. Источники: [сбор настроек](../crates/kronika-source-pg/src/settings.rs), [выбор сведений для Passport](../bins/kronika-web/ui/src/postgres-vitals.ts).

Поля `stats_reset` в секциях баз, WAL, ввода-вывода, фоновой записи и контрольных точек, архиватора и сведений о расширениях содержат серверное время сброса статистики. `stats_since` — начало накопления статистики строки запроса, если поле есть в её формате. `first_call` и `last_call` — время первого и последнего выполнения записанного плана. Интерфейс показывает эти абсолютные метки времени; время, прошедшее после сброса, отдельно не вычисляется.

`pg_wal_storage.wal_files_bytes = COALESCE(SUM(size), 0)` — сумма размеров обычных файлов, возвращённых `pg_ls_waldir()` на момент `statement_timestamp()` этого запроса. Значение отвечает на вопрос, сколько байтов сейчас занимают эти файлы. Они включаются по возвращённому размеру без отбора по именам. Вложенные каталоги не обходятся; при отсутствии права вызова функции секция не записывается. Источник: [запрос размера WAL](../crates/kronika-source-pg/src/wal_storage.rs).

<a id="правила-maintenance-postgresql"></a>

### Правила запуска VACUUM и ANALYZE в PostgreSQL

Следующие формулы определяют, когда сервер PostgreSQL рассматривает таблицу для автоматического VACUUM или ANALYZE. Kronika записывает статистику таблиц и настройки подключения, перечисленные выше, но не собирает `reloptions`, `relpages` и `relallfrozen`. Поэтому готовность конкретной таблицы к такому запуску в интерфейсе не вычисляется.

Пусть `N = max(reltuples,0)` — оценка числа строк, `D = n_dead_tup` — оценка числа мёртвых строк, `I = n_ins_since_vacuum` — число вставленных строк после последнего VACUUM, `M = n_mod_since_analyze` — число изменённых строк после последнего ANALYZE. `V₀/Vₛ`, `I₀/Iₛ`, `A₀/Aₛ` обозначают действующие пары порога и коэффициента: `autovacuum_vacuum_threshold/scale_factor`, `autovacuum_vacuum_insert_threshold/scale_factor`, `autovacuum_analyze_threshold/scale_factor`. Параметр хранения таблицы переопределяет соответствующее общее значение.

| Правило сервера | Формула |
|---|---|
| Dead-tuple vacuum | `D > V₀ + VₛN`; PostgreSQL 18 ограничивает правую часть значением `autovacuum_vacuum_max_threshold`, если оно неотрицательно |
| Insert vacuum, PostgreSQL 13–17 | `I > I₀ + IₛN`; отключён при `I₀ = -1` |
| Insert vacuum, PostgreSQL 18 | `I > I₀ + IₛNf`, где `f = 1 − min(relallfrozen,relpages)/relpages` при положительных значениях relallfrozen и relpages, иначе `f = 1`; отключён при `I₀ = -1` |
| Analyze | `M > A₀ + AₛN` |
| XID / MXID forced vacuum | `age(relfrozenxid) > Xmax` или `mxid_age(relminmxid) > MXmax` для допустимых идентификаторов; серверные сравнения учитывают циклическое переполнение |

`Xmax` — меньшее из порога возраста заморозки, заданного для таблицы, и общего максимума; без настройки таблицы используется общий максимум. `MXmax` аналогично ограничивает значение таблицы действующим максимумом возраста multixact на сервере; тот может быть ниже максимума из конфигурации. Точная реализация порогов находится в функции `relation_needs_vacanalyze`: [PostgreSQL 17](https://github.com/postgres/postgres/blob/REL_17_STABLE/src/backend/postmaster/autovacuum.c), [PostgreSQL 18](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/postmaster/autovacuum.c).

Правило VACUUM по числу вставленных строк появилось в PostgreSQL 13. Определения по версиям: [PostgreSQL 13](https://www.postgresql.org/docs/13/routine-vacuuming.html#AUTOVACUUM), [PostgreSQL 17](https://www.postgresql.org/docs/17/routine-vacuuming.html#AUTOVACUUM), [PostgreSQL 18](https://www.postgresql.org/docs/18/routine-vacuuming.html#AUTOVACUUM). Обслуживание из-за возраста идентификаторов выполняется и при отключённом обычном autovacuum. Заморозка продвигает `relfrozenxid` и `relminmxid`; VACUUM освобождает место строк для повторного использования и поддерживает сведения об их видимости, а ANALYZE обновляет статистику планировщика. Это правила сервера. Пунктирные границы Overview показывают записанные настройки, а отметка сводки в 1,6 миллиарда XID имеет фиксированное значение.
