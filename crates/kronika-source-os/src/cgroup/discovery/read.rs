//! Reads anchored to an already opened cgroup directory.

use super::{
    BufRead, BufReader, CpuQuota, DiscoveredCpu, DiscoveredGroup, DiscoveredIo, DiscoveredMemory,
    DiscoveredPids, DiscoveryRow, DiscoveryStats, File, MAX_IO_LINE_BYTES, MAX_METRIC_BYTES, Mode,
    OFlags, Read, io, openat, parse_cpu_max_strict, parse_cpuset_count,
};

fn open_metric(
    directory: &File,
    path: &str,
    name: &str,
    stats: &mut DiscoveryStats,
) -> Option<File> {
    match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(file) => {
            let file = File::from(file);
            if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
                stats.metric_error(path, name, &io::Error::other("not a regular resource file"));
                return None;
            }
            stats.metric_files_read += 1;
            Some(file)
        }
        Err(error) => {
            let error = io::Error::from(error);
            if error.kind() != io::ErrorKind::NotFound {
                stats.metric_error(path, name, &error);
            }
            None
        }
    }
}

fn text(directory: &File, path: &str, name: &str, stats: &mut DiscoveryStats) -> Option<String> {
    let file = open_metric(directory, path, name, stats)?;
    let mut value = String::new();
    if let Err(error) = file
        .take((MAX_METRIC_BYTES + 1) as u64)
        .read_to_string(&mut value)
    {
        stats.metric_error(path, name, &error);
        return None;
    }
    if value.len() > MAX_METRIC_BYTES {
        stats.metric_error(
            path,
            name,
            &io::Error::other("resource file exceeds 64 KiB"),
        );
        return None;
    }
    Some(value)
}

pub(super) fn scalar<T>(
    directory: &File,
    path: &str,
    name: &str,
    stats: &mut DiscoveryStats,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Option<T> {
    let content = text(directory, path, name, stats)?;
    let value = parse(&content);
    if value.is_none() {
        stats.metric_error(path, name, &io::Error::other("invalid resource value"));
    }
    value
}

fn fields(
    directory: &File,
    path: &str,
    name: &str,
    keys: &[&str],
    stats: &mut DiscoveryStats,
) -> Option<String> {
    let content = text(directory, path, name, stats)?;
    if keys.iter().any(|key| {
        content
            .lines()
            .any(|line| line.split_whitespace().next() == Some(key))
            && value(Some(&content), key).is_none()
    }) {
        stats.metric_error(path, name, &io::Error::other("invalid resource counter"));
    }
    Some(content)
}

fn number(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|value| *value >= 0)
}

pub(super) fn limit(value: &str) -> Option<i64> {
    if value.trim() == "max" {
        Some(-1)
    } else {
        number(value)
    }
}

fn value(text: Option<&str>, key: &str) -> Option<i64> {
    super::super::parse_exact_stat_value(text?, key).filter(|value| *value >= 0)
}

pub(super) fn group(
    directory: &File,
    mut group: DiscoveredGroup,
    stats: &mut DiscoveryStats,
    cached: Option<&super::primary::Limits>,
) -> DiscoveredGroup {
    let path = &group.cgroup_path;
    let cpu = fields(
        directory,
        path,
        "cpu.stat",
        &[
            "usage_usec",
            "user_usec",
            "system_usec",
            "nr_periods",
            "nr_throttled",
            "throttled_usec",
        ],
        stats,
    );
    let cpu_max = cached.map_or_else(
        || scalar(directory, path, "cpu.max", stats, parse_cpu_max_strict),
        |limits| limits.cpu,
    );
    let pair = cpu_max.map(CpuQuota::pair);

    group.cpu = DiscoveredCpu {
        usage_usec: value(cpu.as_deref(), "usage_usec"),
        user_usec: value(cpu.as_deref(), "user_usec"),
        system_usec: value(cpu.as_deref(), "system_usec"),
        nr_periods: value(cpu.as_deref(), "nr_periods"),
        nr_throttled: value(cpu.as_deref(), "nr_throttled"),
        throttled_usec: value(cpu.as_deref(), "throttled_usec"),
        quota_usec: pair.map(|pair| pair.0),
        period_usec: pair.map(|pair| pair.1),
        cpuset_cpus: cached.filter(|limits| limits.cpuset_read).map_or_else(
            || {
                scalar(
                    directory,
                    path,
                    "cpuset.cpus.effective",
                    stats,
                    parse_cpuset_count,
                )
            },
            |limits| limits.cpuset,
        ),
    };
    let memory = fields(
        directory,
        path,
        "memory.stat",
        &["anon", "file", "kernel", "slab"],
        stats,
    );
    let events = fields(
        directory,
        path,
        "memory.events",
        &["low", "high", "max", "oom", "oom_kill"],
        stats,
    );
    let local = fields(
        directory,
        path,
        "memory.events.local",
        &["high", "max", "oom", "oom_kill", "oom_group_kill"],
        stats,
    );
    group.memory = DiscoveredMemory {
        current: scalar(directory, path, "memory.current", stats, number),
        max: cached.map_or_else(
            || scalar(directory, path, "memory.max", stats, limit),
            |limits| limits.memory,
        ),
        high: scalar(directory, path, "memory.high", stats, limit),
        anon: value(memory.as_deref(), "anon"),
        file: value(memory.as_deref(), "file"),
        kernel: value(memory.as_deref(), "kernel"),
        slab: value(memory.as_deref(), "slab"),
        low_events: value(events.as_deref(), "low"),
        high_events: value(events.as_deref(), "high"),
        max_events: value(events.as_deref(), "max"),
        oom_events: value(events.as_deref(), "oom"),
        oom_kill: value(events.as_deref(), "oom_kill"),
        local_high_events: value(local.as_deref(), "high"),
        local_max_events: value(local.as_deref(), "max"),
        local_oom_events: value(local.as_deref(), "oom"),
        local_oom_kill: value(local.as_deref(), "oom_kill"),
        local_oom_group_kill: value(local.as_deref(), "oom_group_kill"),
    };
    group.pids = pids(directory, path, stats);
    group
}

fn pids(directory: &File, path: &str, stats: &mut DiscoveryStats) -> DiscoveredPids {
    let (events_source, events) = fields(directory, path, "pids.events.local", &["max"], stats)
        .map_or_else(
            || {
                let events = fields(directory, path, "pids.events", &["max"], stats);
                (if events.is_some() { 2 } else { 0 }, events)
            },
            |events| (1, Some(events)),
        );
    DiscoveredPids {
        current: scalar(directory, path, "pids.current", stats, number),
        max: scalar(directory, path, "pids.max", stats, limit),
        failure_max: value(events.as_deref(), "max"),
        events_source,
    }
}

pub(super) fn io(
    directory: &File,
    group: &DiscoveredGroup,
    stats: &mut DiscoveryStats,
    emit: &mut impl FnMut(DiscoveryRow<'_>) -> io::Result<()>,
) -> io::Result<()> {
    io_rows(
        directory,
        group.ts,
        &group.cgroup_path,
        &group.cgroup_identity,
        stats,
        emit,
    )
}

pub(super) fn io_rows(
    directory: &File,
    ts: i64,
    path: &str,
    identity: &str,
    stats: &mut DiscoveryStats,
    emit: &mut impl FnMut(DiscoveryRow<'_>) -> io::Result<()>,
) -> io::Result<()> {
    let Some(file) = open_metric(directory, path, "io.stat", stats) else {
        return Ok(());
    };
    let mut input = BufReader::new(file);
    let mut line = Vec::with_capacity(MAX_IO_LINE_BYTES);
    loop {
        line.clear();
        let result = read_line(&mut input, &mut line);
        match result {
            Ok(false) => return Ok(()),
            Err(error) => {
                stats.metric_error(path, "io.stat", &error);
                return Ok(());
            }
            Ok(true) => {}
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            stats.metric_error(path, "io.stat", &io::Error::other("non-UTF-8 device row"));
            continue;
        };
        let mut fields = text.split_whitespace();
        let Some((major, minor)) = fields.next().and_then(|device| device.split_once(':')) else {
            if !text.trim().is_empty() {
                stats.metric_error(
                    path,
                    "io.stat",
                    &io::Error::other("invalid device identifier"),
                );
            }
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse(), minor.parse()) else {
            stats.metric_error(
                path,
                "io.stat",
                &io::Error::other("invalid device identifier"),
            );
            continue;
        };
        let mut row = DiscoveredIo {
            ts,
            cgroup_path: path.to_owned(),
            cgroup_identity: identity.to_owned(),
            major,
            minor,
            rbytes: None,
            wbytes: None,
            rios: None,
            wios: None,
        };
        let mut invalid = false;
        for field in fields {
            let Some((key, value)) = field.split_once('=') else {
                invalid = true;
                continue;
            };
            let destination = match key {
                "rbytes" => &mut row.rbytes,
                "wbytes" => &mut row.wbytes,
                "rios" => &mut row.rios,
                "wios" => &mut row.wios,
                _ => continue,
            };
            *destination = number(value);
            invalid |= destination.is_none();
        }
        if invalid {
            stats.metric_error(path, "io.stat", &io::Error::other("invalid device counter"));
        }
        if row.rbytes.is_some() || row.wbytes.is_some() || row.rios.is_some() || row.wios.is_some()
        {
            emit(DiscoveryRow::Io(&row))?;
            stats.io_rows += 1;
        }
    }
}

fn read_line(input: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if line.len() + count > MAX_IO_LINE_BYTES {
            return Err(io::Error::other("device row exceeds 4096 bytes"));
        }
        line.extend_from_slice(&available[..count]);
        input.consume(count);
        if newline.is_some() {
            return Ok(true);
        }
    }
}
