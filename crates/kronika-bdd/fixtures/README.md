# BDD fixtures

[Русская версия](README.ru.md)

Each directory is a procfs root that a scenario selects through
`KRONIKA_PROC_ROOT`. A fixture contains the files that the scenario reads.

`procfs-without-meminfo` holds the minimum the collector needs to start and
write a segment. It omits `meminfo` and `vmstat`.
