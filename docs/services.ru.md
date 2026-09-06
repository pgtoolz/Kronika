# Сервисы systemd

[English version](services.md) · [Установка](../INSTALL.ru.md)

Systemd запускает сборщик и веб-сервер автоматически и перезапускает их при сбое.
В примере программы установлены в `/usr/local/bin`, запись принадлежит root,
а веб-сервер принимает подключения только с этой машины. С одним каталогом записи
могут работать один сборщик и один веб-сервер, создающий индексы. Перед запуском
сервисов остановите экземпляры, запущенные вручную в терминале.

<a id="environment-files"></a>
## Файлы настроек

Создайте и отредактируйте файлы:

```sh
sudo install -d -m 0700 /etc/kronika /var/lib/kronika
sudo touch /etc/kronika/collector.env /etc/kronika/web.env
sudo chmod 0600 /etc/kronika/collector.env /etc/kronika/web.env
sudoedit /etc/kronika/collector.env /etc/kronika/web.env
```

`/etc/kronika/collector.env`:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_RETENTION=2147483648
```

`/etc/kronika/web.env` — замените пароль:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_WEB_LISTEN=127.0.0.1:8080
KRONIKA_WEB_SOURCES=1
KRONIKA_WEB_USER=kronika
KRONIKA_WEB_PASSWORD=replace-with-a-random-password
```

Каждая строка задаёт переменную окружения для программы. Значения с пробелами
целиком заключаются в кавычки. Systemd не исполняет здесь команды оболочки: не
используйте `export` и подстановки команд.

После [создания роли PostgreSQL](../INSTALL.ru.md#5-postgresql) добавьте в `collector.env`:

```ini
KRONIKA_PG_DSNS="host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres"
```

Для PostgreSQL в той же виртуальной машине или контейнере с теми же
ограничениями ресурсов не задавайте `KRONIKA_POSTGRES_EFFECTIVE_CPUS`. Для
удалённого PostgreSQL или другой cgroup — группы процессов с общими
ограничениями ресурсов — укажите доступное серверу число CPU положительным
целым числом, например `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`.
[Как определяется доступное число CPU](metrics-time.ru.md#health).
В `web.env` задайте `KRONIKA_WEB_SOURCES=3`, чтобы отметить Linux и PostgreSQL
как настроенные источники. Полный список параметров:
[collector](../bins/kronika-collector/README.ru.md) и
[веб-сервер](../bins/kronika-web/README.ru.md).

<a id="units"></a>
## Описание сервисов

Создайте эти файлы через `sudoedit`:

`/etc/systemd/system/kronika-collector.service`:

```ini
[Unit]
Description=Kronika machine history collector
After=network.target

[Service]
Type=simple
User=root
UMask=0077
EnvironmentFile=/etc/kronika/collector.env
ExecStart=/usr/local/bin/kronika-collector
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

`/etc/systemd/system/kronika-web.service`:

```ini
[Unit]
Description=Kronika history web and MCP
After=network.target

[Service]
Type=simple
User=root
UMask=0077
EnvironmentFile=/etc/kronika/web.env
ExecStart=/usr/local/bin/kronika-web
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Оба сервиса используют `UMask=0077`: создаваемые файлы закрыты для других
пользователей. Сборщик читает защищённые данные процессов и журналы; веб-сервер
создаёт индексы и файлы блокировки в том же каталоге записи. Веб-сервер может читать
запись и после остановки сборщика. Экспорт создаёт временные файлы ZMS и HTML
в `TMPDIR` или системном временном каталоге.

## Запуск

```sh
sudo systemd-analyze verify /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now kronika-collector.service kronika-web.service
sudo systemctl status kronika-collector.service kronika-web.service
sudo journalctl -u kronika-collector -u kronika-web --since '5 minutes ago'
```

Откройте <http://127.0.0.1:8080/>. По тому же адресу с путём `/mcp` доступен MCP.
[Перенаправление порта по SSH](../INSTALL.ru.md#4-запуск-web) позволяет
подключиться с другой машины.

## Операции

| Операция | Команда |
| --- | --- |
| Журнал сборщика | `sudo journalctl -u kronika-collector -f` |
| Журнал веб-сервера | `sudo journalctl -u kronika-web -f` |
| Немедленный сбор; сохранение сегмента, если добавлены данные и сегмент непустой | `sudo systemctl kill --kill-whom=main --signal=SIGUSR2 kronika-collector` |
| Применить изменения настроек | `sudo systemctl restart kronika-collector kronika-web` |
| Остановка сбора с сохранением файлов | `sudo systemctl stop kronika-collector` |
| Отключение автозапуска и остановка веб-сервера | `sudo systemctl disable --now kronika-web` |
| Запуск веб-сервера | `sudo systemctl start kronika-web` |
| Место, занятое записью | `sudo du -sh /var/lib/kronika` |

<a id="замена-binaries"></a>
## Замена программ

Проверьте и распакуйте следующий архив по [инструкции установки](../INSTALL.ru.md#1-скачивание-и-распаковка).
В распакованном каталоге:

```sh
for binary in kronika-collector kronika-web kronika-dump kronika-report; do
  "./$binary" --version
done
sudo systemctl stop kronika-collector kronika-web
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
sudo systemctl start kronika-collector kronika-web
```

Конфигурация остаётся в `/etc/kronika`; записи остаются в `/var/lib/kronika`.

## Удаление сервисов

```sh
sudo systemctl disable --now kronika-collector kronika-web
sudo rm /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
```

Команды удаляют описания двух сервисов. Программы, настройки и записи остаются.
