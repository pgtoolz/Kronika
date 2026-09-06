# Captured-hour interface fixture

[Русская версия](README.ru.md)

`real-hour.json.gz` contains the data recovered from a saved standalone HTML interface. It retains the captured OS process rows,
PostgreSQL activity rows, exact PID relationships, one-hour system series and
finding locators. It is used only by interface tests and the opt-in standalone
demo build; `kronika-web` does not ship the captured rows.

The data is compressed reproducibly with gzip, without an original filename. `real-hour.manifest.json` records
its source and content hashes. `npm run fixture:check` validates structure,
record counts, reproducible compression and patterns for credentials, connection
strings, authorization headers and keys without printing command lines or SQL text.

To reproduce the fixture from the preserved HTML:

```sh
node scripts/real-fixture.mjs --recover /path/to/index.html
```

To build a temporary, self-contained demo from the same React source:

```sh
node scripts/build.mjs --fixture-output /absolute/path/kronika-real-hour.html.gz
```
