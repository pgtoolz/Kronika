# BDD fixtures

[Русская версия](README.ru.md)

Each directory contains ordinary files arranged like procfs, Linux's `/proc`
filesystem. A BDD scenario sets `KRONIKA_PROC_ROOT` to this directory so the
collector reads the test files instead of the host's `/proc`.

`procfs-without-meminfo` holds the minimum the collector needs to start and
write a segment. It omits `meminfo` and `vmstat`.
