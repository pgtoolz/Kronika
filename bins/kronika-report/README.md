# kronika-report

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

`kronika-report` turns a `.zms` recording into an interactive HTML report you
can open directly in a browser. Use a finished collector segment or a file
created by `kronika-dump slice`. Sources: [command and argument validation](src/main.rs),
[parameter help](src/help.rs).

```sh
kronika-report incident.zms incident.html
```

## Parameters

| Parameter | Default | Contract |
| --- | --- | --- |
| `INPUT.zms` | Required | Valid finished standalone ZMS, with any basename. |
| `OUTPUT.html` | Required | Exact `.html` path; existing output is atomically replaced. Parent directory must exist and be writable. |
| `--from MICROSECONDS` | First recorded microsecond | Inclusive beginning of visible navigation window. |
| `--to-exclusive MICROSECONDS` | Last recorded microsecond + 1 | Exclusive end of visible navigation window. |
| `-h`, `--help` | — | Parameter reference; exits before file access. |
| `--version` | — | Binary version; exits before file access. |

Explicit bounds are supplied together in this order, before both paths, and
satisfy `0 < from < to-exclusive <= 9007199254740991`. Units are whole Unix
microseconds. The visible interval is `[from, to-exclusive)`.

## Exact report interval

```sh
kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

This selects 5 September 2026, 19:00–20:00 UTC. A
[sliced ZMS](../kronika-dump/README.md#slice) can retain nearby samples for
interval calculations; explicit bounds restrict report navigation while those
samples remain available to the query engine. A first rate requiring an earlier
sample remains null when that sample is absent.

## Output and execution

The command validates the ZMS, derives its internal segment identity, builds
the canonical IDX, and embeds the ZMS/IDX, production interface and
Rust/WebAssembly query engine into HTML. The engine runs on the browser's
main thread. Tables, heatmaps, search and charts execute locally; the interface
has no authentication, MCP, live refresh or Export control.

Temporary HTML is written beside `OUTPUT.html`; no IDX sidecar is created.
Report configuration reads no environment variables. Success exits 0 with
empty stdout; errors go to stderr and exit nonzero. `Ctrl+C` interrupts the
conversion. Web's **Export** creates this HTML and supplies its visible bounds
from the selected interval.
