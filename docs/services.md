# Systemd services

[Русская версия](services.ru.md) · [Install](../INSTALL.md)

You can launch the programs however you prefer. This guide shows automatic
startup and restart using systemd. The example runs one collector and one web
process, using `/usr/local/bin`, root-owned storage and a web listener on
all IPv4 interfaces. Stop any manually started instance before starting its service.

## Environment files

Create and edit the files:

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

`/etc/kronika/web.env`:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_WEB_LISTEN=0.0.0.0:8080
KRONIKA_WEB_SOURCES=1
```

With `KRONIKA_WEB_USER` and `KRONIKA_WEB_PASSWORD` both unset, the browser,
API and MCP require no authentication. To require it, add both lines to
`web.env`, choosing your own password:

```ini
KRONIKA_WEB_USER=kronika
KRONIKA_WEB_PASSWORD=replace-with-a-random-password
```

Setting only one credential or an empty value prevents startup.

Systemd parses these as environment assignments. Values containing spaces are
quoted as a whole; shell substitutions and `export` are not evaluated.

For Linux-only collection, these settings are sufficient. To also collect
PostgreSQL, [prepare a monitoring role](../INSTALL.md#5-postgresql) and add its
connection string to `collector.env`:

```ini
KRONIKA_PG_DSN="host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres"
```

For PostgreSQL on the collector machine, use the connection above and set
`KRONIKA_WEB_SOURCES=3` in `web.env` to declare Linux and PostgreSQL.

For PostgreSQL-only collection, set `KRONIKA_COLLECTOR_MODE=postgresql` in
`collector.env` and `KRONIKA_WEB_SOURCES=2` in `web.env`. PostgreSQL may be local
or remote. Collector mode controls recording; the web setting declares sources
in the catalog. See [connection settings](../bins/kronika-collector/README.md#remote-postgresql).
All parameters:
[collector](../bins/kronika-collector/README.md) and
[web](../bins/kronika-web/README.md).

To collect from several PostgreSQL servers, run a `kronika-collector` process
for each server with its DSN and a separate storage directory. Each web process
reads one storage directory and needs its own listen address. See the
[two-server example](../bins/kronika-collector/README.md#several-postgresql-servers).

## Units

Create these files with `sudoedit`:

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

`UMask=0077` keeps new files private. Web needs write access to storage for
indexes and can serve recordings after the collector stops.

## Start

```sh
sudo systemd-analyze verify /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now kronika-collector.service kronika-web.service
sudo systemctl status kronika-collector.service kronika-web.service
sudo journalctl -u kronika-collector -u kronika-web --since '5 minutes ago'
```

Open `http://<server-ip>:8080`, replacing `<server-ip>` with the server's
address. The same listener serves `/mcp`. For local access, a reverse proxy on
the same machine or [SSH forwarding](../INSTALL.md#4-start-web), use
`KRONIKA_WEB_LISTEN=127.0.0.1:8080` in `web.env` instead.

## Operations

After editing an environment file, restart the affected service. For example,
to add PostgreSQL collection and change the web source setting:

```sh
sudoedit /etc/kronika/collector.env /etc/kronika/web.env
sudo systemctl restart kronika-collector kronika-web
```

| Operation | Command |
| --- | --- |
| Collector log | `sudo journalctl -u kronika-collector -f` |
| Web log | `sudo journalctl -u kronika-web -f` |
| Collect now and save the segment if this collection adds data | `sudo systemctl kill --kill-whom=main --signal=SIGUSR2 kronika-collector` |
| Apply environment changes | `sudo systemctl restart kronika-collector kronika-web` |
| Stop collection | `sudo systemctl stop kronika-collector` |
| Disable web startup and stop web | `sudo systemctl disable --now kronika-web` |
| Start web | `sudo systemctl start kronika-web` |
| Storage bytes | `sudo du -sh /var/lib/kronika` |

## Replace binaries

Download and extract the next archive using [Install](../INSTALL.md#1-download-and-extract).
From its extracted directory:

```sh
sudo systemctl stop kronika-collector kronika-web
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
sudo systemctl start kronika-collector kronika-web
```

Configuration remains in `/etc/kronika`; recordings remain in `/var/lib/kronika`.

## Remove services

```sh
sudo systemctl disable --now kronika-collector kronika-web
sudo rm /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
```

This removes the two units. Binaries, configuration and recordings remain.
