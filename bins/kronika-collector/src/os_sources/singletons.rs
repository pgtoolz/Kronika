//! The one-row-per-tick OS files: CPU, memory, load, paging, pressure.

use super::{
    Instant, LogLevel, OsScope, OsSources, ProcFs, SysFs, cgroup, field, layout_id,
    log_collection_finish, log_event, parse_cpu, parse_loadavg, parse_meminfo, parse_pressure,
    parse_stat_misc, parse_vmstat, read_optional_os_file, section_name,
};

/// Read the singleton procfs files into `os`.
pub(super) fn collect_singletons(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    in_container: bool,
    os: &mut OsSources,
) {
    collect_cpu_and_stat(fs, scope, ts, os);
    collect_memory_and_load(fs, scope, ts, os);
    collect_paging(fs, scope, ts, os);
    collect_pressure_rows(fs, sys, scope, ts, in_container, os);
}

/// `/proc/stat`: the CPU lines and the singleton counters beside them.
fn collect_cpu_and_stat(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    // stat — read once, feed to both cpu and stat-misc parsers.
    let stat_started = Instant::now();
    match fs.read("stat") {
        Ok(content) => {
            // CPU rows (1_102_001)
            let cpu_type_id = 1_102_001_u32;
            match parse_cpu(&content, ts) {
                Ok(rows) => {
                    let n = rows.len();
                    os.cpu = rows.into_iter().map(|r| r.to_section(scope)).collect();
                    log_collection_finish(cpu_type_id, "procfs", n, stat_started.elapsed());
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "collection_degraded",
                        &[
                            field("collection", section_name(cpu_type_id)),
                            field("type_id", cpu_type_id),
                            field("layout_id", layout_id(cpu_type_id)),
                            field("source", "stat"),
                            field("reason", &err.0),
                        ],
                    );
                }
            }
            // Stat-misc row (1_103_001) — same content, separate parser.
            // Its own clock so the reported latency excludes the CPU parse above.
            let stat_misc_started = Instant::now();
            let stat_type_id = 1_103_001_u32;
            match parse_stat_misc(&content, ts) {
                Ok(row) => {
                    os.stat = Some(row.to_section(scope));
                    log_collection_finish(stat_type_id, "procfs", 1, stat_misc_started.elapsed());
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "collection_degraded",
                        &[
                            field("collection", section_name(stat_type_id)),
                            field("type_id", stat_type_id),
                            field("layout_id", layout_id(stat_type_id)),
                            field("source", "stat"),
                            field("reason", &err.0),
                        ],
                    );
                }
            }
        }
        Err(err) => {
            let cpu_type_id = 1_102_001_u32;
            let stat_type_id = 1_103_001_u32;
            log_event(
                LogLevel::Warn,
                "collection_degraded",
                &[
                    field("collection", section_name(cpu_type_id)),
                    field("type_id", cpu_type_id),
                    field("layout_id", layout_id(cpu_type_id)),
                    field("source", "stat"),
                    field("reason", &err),
                ],
            );
            log_event(
                LogLevel::Warn,
                "collection_degraded",
                &[
                    field("collection", section_name(stat_type_id)),
                    field("type_id", stat_type_id),
                    field("layout_id", layout_id(stat_type_id)),
                    field("source", "stat"),
                    field("reason", &err),
                ],
            );
        }
    }
}

/// `/proc/meminfo`, `/proc/loadavg`, `/proc/vmstat` and `/proc/pressure/*`.
/// `/proc/meminfo` and `/proc/loadavg`.
fn collect_memory_and_load(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    // meminfo (1_104_001)
    {
        let type_id = 1_104_001_u32;
        let started = Instant::now();
        match fs.read("meminfo") {
            Ok(content) => match parse_meminfo(&content, ts) {
                Ok(row) => {
                    os.meminfo = Some(row.to_section(scope));
                    log_collection_finish(type_id, "procfs", 1, started.elapsed());
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "collection_degraded",
                        &[
                            field("collection", section_name(type_id)),
                            field("type_id", type_id),
                            field("layout_id", layout_id(type_id)),
                            field("source", "meminfo"),
                            field("reason", &err.0),
                        ],
                    );
                }
            },
            Err(err) => {
                log_event(
                    LogLevel::Warn,
                    "collection_degraded",
                    &[
                        field("collection", section_name(type_id)),
                        field("type_id", type_id),
                        field("layout_id", layout_id(type_id)),
                        field("source", "meminfo"),
                        field("reason", &err),
                    ],
                );
            }
        }
    }

    // loadavg (1_105_001)
    {
        let type_id = 1_105_001_u32;
        let started = Instant::now();
        match fs.read("loadavg") {
            Ok(content) => match parse_loadavg(&content, ts) {
                Ok(row) => {
                    os.loadavg = Some(row.to_section(scope));
                    log_collection_finish(type_id, "procfs", 1, started.elapsed());
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "collection_degraded",
                        &[
                            field("collection", section_name(type_id)),
                            field("type_id", type_id),
                            field("layout_id", layout_id(type_id)),
                            field("source", "loadavg"),
                            field("reason", &err.0),
                        ],
                    );
                }
            },
            Err(err) => {
                log_event(
                    LogLevel::Warn,
                    "collection_degraded",
                    &[
                        field("collection", section_name(type_id)),
                        field("type_id", type_id),
                        field("layout_id", layout_id(type_id)),
                        field("source", "loadavg"),
                        field("reason", &err),
                    ],
                );
            }
        }
    }
}

/// `/proc/vmstat`.
fn collect_paging(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    // vmstat (1_106_001)
    {
        let type_id = 1_106_001_u32;
        let started = Instant::now();
        match fs.read("vmstat") {
            Ok(content) => match parse_vmstat(&content, ts) {
                Ok(row) => {
                    os.vmstat = Some(row.to_section(scope));
                    log_collection_finish(type_id, "procfs", 1, started.elapsed());
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "collection_degraded",
                        &[
                            field("collection", section_name(type_id)),
                            field("type_id", type_id),
                            field("layout_id", layout_id(type_id)),
                            field("source", "vmstat"),
                            field("reason", &err.0),
                        ],
                    );
                }
            },
            Err(err) => {
                log_event(
                    LogLevel::Warn,
                    "collection_degraded",
                    &[
                        field("collection", section_name(type_id)),
                        field("type_id", type_id),
                        field("layout_id", layout_id(type_id)),
                        field("source", "vmstat"),
                        field("reason", &err),
                    ],
                );
            }
        }
    }
}

/// Host `/proc/pressure` or the collector's exact cgroup v2 pressure files.
pub(super) fn collect_pressure_rows(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    in_container: bool,
    os: &mut OsSources,
) {
    let type_id = 1_107_001_u32;
    let started = Instant::now();
    let (source, timing_source, pressure_scope, rows) = if in_container {
        (
            "cgroup/{cpu,memory,io}.pressure",
            "cgroup",
            OsScope::Container.as_u8(),
            cgroup::collect_pressure(fs, sys, ts),
        )
    } else {
        let psi_cpu = read_optional_os_file(fs, "pressure/cpu", type_id);
        let psi_memory = read_optional_os_file(fs, "pressure/memory", type_id);
        let psi_io = read_optional_os_file(fs, "pressure/io", type_id);
        (
            "pressure/{cpu,memory,io}",
            "procfs",
            scope,
            parse_pressure(
                psi_cpu.as_deref(),
                psi_memory.as_deref(),
                psi_io.as_deref(),
                ts,
            ),
        )
    };

    match rows {
        Ok(rows) => {
            let n = rows.len();
            if n == 0 {
                log_event(
                    LogLevel::Warn,
                    "collection_degraded",
                    &[
                        field("collection", section_name(type_id)),
                        field("type_id", type_id),
                        field("layout_id", layout_id(type_id)),
                        field("source", source),
                        field("reason", "no pressure files available"),
                    ],
                );
            } else {
                os.psi = rows
                    .into_iter()
                    .map(|row| row.to_section(pressure_scope))
                    .collect();
                log_collection_finish(type_id, timing_source, n, started.elapsed());
            }
        }
        Err(err) => {
            log_event(
                LogLevel::Warn,
                "collection_degraded",
                &[
                    field("collection", section_name(type_id)),
                    field("type_id", type_id),
                    field("layout_id", layout_id(type_id)),
                    field("source", source),
                    field("reason", &err.0),
                ],
            );
        }
    }
}
