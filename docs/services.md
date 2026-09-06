# Systemd services

[Русская версия](services.ru.md) · [Install](../INSTALL.md)

Systemd starts collector and web automatically and restarts them after a
failure. These service files use programs in `/usr/local/bin`, root-owned
storage and a web server accepting local connections only. Each recording
directory permits one collector and one web process that creates indexes.
Stop any copies running in terminals before starting the services.

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

`/etc/kronika/web.env` — replace the password:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_WEB_LISTEN=127.0.0.1:8080
KRONIKA_WEB_SOURCES=1
KRONIKA_WEB_USER=kronika
KRONIKA_WEB_PASSWORD=replace-with-a-random-password
```

Systemd parses these as environment assignments. Values containing spaces are
quoted as a whole; shell substitutions and `export` are not evaluated.

After [PostgreSQL role setup](../INSTALL.md#5-postgresql), add to `collector.env`:

```ini
KRONIKA_PG_DSNS="host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres"
```

Leave `KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset for local PostgreSQL in the same
VM/container resource scope. For remote PostgreSQL or a different cgroup, set
the target server's positive whole CPU capacity, for example
`KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`. [Capacity calculation](metrics-time.md#health).
`KRONIKA_WEB_SOURCES=3` marks OS and PostgreSQL
configured in the web catalog. All parameters:
[collector](../bins/kronika-collector/README.md) and
[web](../bins/kronika-web/README.md).

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

Both services use `UMask=0077`. Collector reads protected process and log data;
web creates indexes and ownership locks in the same storage root. Web also
reads recordings after collector stops. Export creates temporary ZMS/HTML files
in `TMPDIR` or the system temporary directory.

## Start

```sh
sudo systemd-analyze verify /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now kronika-collector.service kronika-web.service
sudo systemctl status kronika-collector.service kronika-web.service
sudo journalctl -u kronika-collector -u kronika-web --since '5 minutes ago'
```

Open <http://127.0.0.1:8080/>. The same listener serves `/mcp`.
[SSH forwarding](../INSTALL.md#4-start-web) provides access from another machine.

## Operations

| Operation | Command |
| --- | --- |
| Collector log | `sudo journalctl -u kronika-collector -f` |
| Web log | `sudo journalctl -u kronika-web -f` |
| Immediate collection; publication if data was appended and segment is nonempty | `sudo systemctl kill --kill-whom=main --signal=SIGUSR2 kronika-collector` |
| Apply environment changes | `sudo systemctl restart kronika-collector kronika-web` |
| Stop collection and retain files | `sudo systemctl stop kronika-collector` |
| Disable web startup and stop web | `sudo systemctl disable --now kronika-web` |
| Start web | `sudo systemctl start kronika-web` |
| Storage bytes | `sudo du -sh /var/lib/kronika` |

## Replace binaries

Verify and extract the next archive using [Install](../INSTALL.md#1-download-and-extract).
From its extracted directory:

```sh
for binary in kronika-collector kronika-web kronika-dump kronika-report; do
  "./$binary" --version
done
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
