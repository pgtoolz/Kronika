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

/// Read the process's unified cgroup v2 membership from `/proc/PID/cgroup`.
#[must_use]
pub fn parse_cgroup_path(content: &str) -> Option<String> {
    crate::cgroup::parse_unified_cgroup_path(content).map(str::to_owned)
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
#[path = "../../tests/proc/process/parse.rs"]
mod tests;
