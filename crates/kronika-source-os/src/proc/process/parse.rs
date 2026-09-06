//! Parsers for per-process procfs files.

use super::ParseError;
use super::model::{ProcIo, ProcStat, ProcStatus, ProcessFacts};

/// Parse `/proc/PID/stat`.
///
/// # Errors
/// Returns [`ParseError`] when required fields are missing or malformed.
pub fn parse_stat(content: &str) -> Result<ProcStat, ParseError> {
    let content = content.trim();
    let open = content
        .find('(')
        .ok_or_else(|| ParseError("stat: missing '('".to_owned()))?;
    let close = content
        .rfind(')')
        .ok_or_else(|| ParseError("stat: missing ')'".to_owned()))?;
    if close <= open {
        return Err(ParseError("stat: invalid comm parentheses".to_owned()));
    }
    let pid_text = content
        .get(..open)
        .ok_or_else(|| ParseError("stat: invalid pid slice".to_owned()))?;
    let comm = content
        .get(open + '('.len_utf8()..close)
        .ok_or_else(|| ParseError("stat: invalid comm slice".to_owned()))?
        .to_owned();
    let rest = content
        .get(close + ')'.len_utf8()..)
        .ok_or_else(|| ParseError("stat: invalid field slice".to_owned()))?;
    let pid = pid_text
        .trim()
        .parse::<i32>()
        .map_err(|err| ParseError(format!("stat pid: {err}")))?;
    let mut fields = [""; 40];
    let mut source = rest.split_whitespace();
    for (index, field) in fields.iter_mut().enumerate() {
        let Some(value) = source.next() else {
            return Err(ParseError(format!(
                "stat: expected at least 40 fields after comm, got {index}"
            )));
        };
        *field = value;
    }
    Ok(ProcStat {
        pid,
        comm,
        state: fields[0].bytes().next().unwrap_or(b'?'),
        ppid: parse_i32(fields[1], "ppid")?,
        tty_nr: parse_i32(fields[4], "tty_nr")?,
        minflt: parse_i64(fields[7], "minflt")?,
        majflt: parse_i64(fields[9], "majflt")?,
        utime: parse_i64(fields[11], "utime")?,
        stime: parse_i64(fields[12], "stime")?,
        priority: parse_i64(fields[15], "priority")?,
        nice: parse_i64(fields[16], "nice")?,
        num_threads: parse_i64(fields[17], "num_threads")?,
        starttime_ticks: parse_i64(fields[19], "starttime")?,
        vsize_bytes: parse_i64(fields[20], "vsize")?,
        rss_pages: parse_i64(fields[21], "rss")?,
        exit_signal: parse_i64(fields[35], "exit_signal")?,
        processor: parse_i64(fields[36], "processor")?,
        rt_priority: parse_i64(fields[37], "rt_priority")?,
        policy: parse_i64(fields[38], "policy")?,
        delayacct_blkio_ticks: parse_i64(fields[39], "delayacct_blkio_ticks")?,
    })
}

/// Parse `/proc/PID/status`.
///
/// # Errors
/// Returns [`ParseError`] when UID/GID lines are malformed.
pub fn parse_status(content: &str) -> Result<ProcStatus, ParseError> {
    let mut status = ProcStatus::default();
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Uid" => {
                let ids = parse_id_quad(value, "Uid")?;
                status.uid = ids[0];
                status.euid = ids[1];
            }
            "Gid" => {
                let ids = parse_id_quad(value, "Gid")?;
                status.gid = ids[0];
                status.egid = ids[1];
            }
            "VmData" => status.vm_data = parse_kb(value),
            "VmStk" => status.vm_stk = parse_kb(value),
            "VmLib" => status.vm_lib = parse_kb(value),
            "VmSwap" => status.vm_swap = parse_kb(value),
            "VmLck" => status.vm_lck = parse_kb(value),
            "VmPTE" => status.vm_pte = parse_kb(value),
            "VmPeak" => status.vm_peak = parse_kb(value),
            "VmHWM" => status.vm_hwm = parse_kb(value),
            "Threads" => status.threads = value.parse().unwrap_or(0),
            "FDSize" => status.fdsize = value.parse().unwrap_or(0),
            "voluntary_ctxt_switches" => {
                status.voluntary_ctxt_switches = value.parse().unwrap_or(0);
            }
            "nonvoluntary_ctxt_switches" => {
                status.nonvoluntary_ctxt_switches = value.parse().unwrap_or(0);
            }
            _ => {}
        }
    }
    Ok(status)
}

/// Parse `/proc/PID/io`.
#[must_use]
pub fn parse_io(content: &str) -> ProcIo {
    let mut io = ProcIo::default();
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().parse::<i64>().unwrap_or(0);
        match key.trim() {
            "rchar" => io.rchar = value,
            "wchar" => io.wchar = value,
            "syscr" => io.syscr = value,
            "syscw" => io.syscw = value,
            "read_bytes" => io.read_bytes = value,
            "write_bytes" => io.write_bytes = value,
            "cancelled_write_bytes" => io.cancelled_write_bytes = value,
            _ => {}
        }
    }
    io
}

/// Pick one process cgroup path from `/proc/PID/cgroup`.
#[must_use]
pub fn parse_cgroup_path(content: &str) -> Option<String> {
    let mut first = None;
    let mut preferred = None;
    for line in content.lines() {
        let mut parts = line.splitn(3, ':');
        let _hierarchy = parts.next();
        let controllers = parts.next()?;
        let path = normalize_cgroup_path(parts.next()?);
        if controllers.is_empty() {
            return Some(path);
        }
        if first.is_none() {
            first = Some(path.clone());
        }
        if preferred.is_none()
            && controllers
                .split(',')
                .any(|c| matches!(c, "pids" | "cpu" | "cpuacct" | "memory"))
        {
            preferred = Some(path);
        }
    }
    preferred.or(first)
}

fn normalize_cgroup_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        "/".to_owned()
    } else if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

pub(super) fn parse_btime(stat: &str) -> Option<i64> {
    stat.lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|rest| rest.trim().parse::<i64>().ok())
        .and_then(|secs| secs.checked_mul(1_000_000))
}

pub(super) fn process_starttime_usec(facts: ProcessFacts, starttime_ticks: i64) -> i64 {
    if facts.clock_ticks_per_sec <= 0 {
        return facts.btime_usec;
    }
    let delta = i128::from(starttime_ticks).saturating_mul(1_000_000)
        / i128::from(facts.clock_ticks_per_sec);
    i64::try_from(i128::from(facts.btime_usec).saturating_add(delta)).unwrap_or(i64::MAX)
}

pub(super) fn rss_kb(rss_pages: i64, page_size_bytes: i64) -> i64 {
    if rss_pages <= 0 || page_size_bytes <= 0 {
        return 0;
    }
    i64::try_from(i128::from(rss_pages).saturating_mul(i128::from(page_size_bytes)) / 1024)
        .unwrap_or(i64::MAX)
}

pub(super) fn normalize_cmdline(content: &str) -> Option<String> {
    let trimmed =
        content.trim_matches(|character: char| character == '\0' || character.is_whitespace());
    (!trimmed.is_empty()).then(|| trimmed.replace('\0', " "))
}

fn parse_id_quad(value: &str, key: &str) -> Result<[u32; 4], ParseError> {
    let mut ids = [0_u32; 4];
    for (idx, part) in value.split_whitespace().take(4).enumerate() {
        ids[idx] = part
            .parse()
            .map_err(|err| ParseError(format!("{key}[{idx}]: {err}")))?;
    }
    Ok(ids)
}

fn parse_kb(value: &str) -> i64 {
    value
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn parse_i32(value: &str, name: &str) -> Result<i32, ParseError> {
    value
        .parse()
        .map_err(|err| ParseError(format!("stat {name}: {err}")))
}

fn parse_i64(value: &str, name: &str) -> Result<i64, ParseError> {
    value
        .parse()
        .map_err(|err| ParseError(format!("stat {name}: {err}")))
}

pub(super) fn u32_from_i64(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

pub(super) fn i32_from_i64(value: i64) -> i32 {
    i32::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

pub(super) fn i16_from_i64(value: i64) -> i16 {
    i16::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        }
    })
}

pub(super) fn i8_from_i64(value: i64) -> i8 {
    i8::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i8::MIN
        } else {
            i8::MAX
        }
    })
}

pub(super) fn u8_from_i64(value: i64) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_line(comm: &str) -> String {
        format!(
            "123 ({comm}) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 -5 16 17 190 204800 12 21 22 23 24 25 26 27 28 29 30 31 32 33 15 2 7 8 9 10 11 12 13 14 15"
        )
    }

    #[test]
    fn stat_comm_uses_the_last_parenthesis() {
        let row = parse_stat(&stat_line("worker (bg) 1")).expect("stat");
        assert_eq!(row.comm, "worker (bg) 1");
        assert_eq!(row.pid, 123);
        assert_eq!(row.state, b'S');
        assert_eq!(row.ppid, 1);
        assert_eq!(row.minflt, 7);
        assert_eq!(row.majflt, 9);
        assert_eq!(row.utime, 11);
        assert_eq!(row.stime, 12);
        assert_eq!(row.nice, -5);
        assert_eq!(row.starttime_ticks, 190);
        assert_eq!(row.exit_signal, 15);
        assert_eq!(row.processor, 2);
        assert_eq!(row.rt_priority, 7);
        assert_eq!(row.policy, 8);
        assert_eq!(row.delayacct_blkio_ticks, 9);
    }

    #[test]
    fn status_parses_identity_memory_and_switches() {
        let status = parse_status(
            "Uid:\t1000\t1001\t1002\t1003\n\
             Gid:\t2000\t2001\t2002\t2003\n\
             VmData:\t10 kB\nVmStk:\t11 kB\nVmLib:\t12 kB\nVmSwap:\t13 kB\n\
             VmLck:\t14 kB\nVmPTE:\t15 kB\nVmPeak:\t16 kB\nVmHWM:\t17 kB\n\
             Threads:\t3\nFDSize:\t64\n\
             voluntary_ctxt_switches:\t20\nnonvoluntary_ctxt_switches:\t21\n",
        )
        .expect("status");
        assert_eq!((status.uid, status.euid), (1000, 1001));
        assert_eq!((status.gid, status.egid), (2000, 2001));
        assert_eq!(status.vm_swap, 13);
        assert_eq!(status.vm_pte, 15);
        assert_eq!(status.vm_hwm, 17);
        assert_eq!(status.threads, 3);
        assert_eq!(status.fdsize, 64);
        assert_eq!(status.nonvoluntary_ctxt_switches, 21);
    }

    #[test]
    fn io_parser_keeps_all_seven_fields() {
        let io = parse_io(
            "rchar: 1\nwchar: 2\nsyscr: 3\nsyscw: 4\nread_bytes: 5\n\
             write_bytes: 6\ncancelled_write_bytes: 7\n",
        );
        assert_eq!(
            io,
            ProcIo {
                rchar: 1,
                wchar: 2,
                syscr: 3,
                syscw: 4,
                read_bytes: 5,
                write_bytes: 6,
                cancelled_write_bytes: 7,
            }
        );
    }

    #[test]
    fn cgroup_path_prefers_v2_then_known_v1_controllers() {
        assert_eq!(
            parse_cgroup_path("0::/kubepods/pod-a/container\n"),
            Some("/kubepods/pod-a/container".to_owned())
        );
        assert_eq!(
            parse_cgroup_path("1:name=systemd:/x\n4:pids:/docker/abc\n"),
            Some("/docker/abc".to_owned())
        );
    }

    #[test]
    fn starttime_uses_boot_time_and_hz() {
        let facts = ProcessFacts {
            btime_usec: 1_700_000_000_000_000,
            clock_ticks_per_sec: 250,
            page_size_bytes: 8192,
        };
        assert_eq!(process_starttime_usec(facts, 500), 1_700_000_002_000_000);
        assert_eq!(rss_kb(2, facts.page_size_bytes), 16);
    }

    #[test]
    fn cmdline_trims_raw_boundaries_before_replacing_separators() {
        assert_eq!(
            normalize_cmdline(" \tpostgres\0-D\0/data\0\n"),
            Some("postgres -D /data".to_owned())
        );
        assert_eq!(normalize_cmdline("\0 \t\n\0"), None);
    }
}
