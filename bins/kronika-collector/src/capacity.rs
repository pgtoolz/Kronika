//! Bounded filesystem-capacity collection for mount snapshots.
//!
//! `statvfs(2)` runs in a helper process so one stuck mount cannot stop the
//! collector. The parent keeps every complete result written before the one
//! pass-wide deadline and leaves the rest nullable.

use crate::logging::{LogLevel, field, log_event};
use anyhow::{Context, Result};
use kronika_source_os::{FsSpace, MountEntry, statvfs};
use rustix::fs::{MemfdFlags, memfd_create};
use std::fs::File;
use std::io::{Read, Seek as _, SeekFrom, Write};
use std::os::unix::fs::FileExt as _;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

const HELPER_ARGUMENT: &str = "--kronika-statvfs-helper";
const CAPACITY_DEADLINE: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const RECORD_LEN: usize = 1 + 8 + 8 + 8 + 8;
static PENDING_HELPER: Mutex<Option<Child>> = Mutex::new(None);

enum HelperOutcome {
    Finished(ExitStatus),
    TimedOut {
        kill_error: Option<String>,
        reap_error: Option<String>,
        reaped: bool,
    },
    PollFailed {
        error: String,
        kill_error: Option<String>,
        reap_error: Option<String>,
        reaped: bool,
    },
}

/// Whether this process invocation is the private capacity helper.
pub(crate) fn is_helper_invocation() -> bool {
    std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == HELPER_ARGUMENT)
}

/// Run the private capacity helper protocol on stdin/stdout.
///
/// # Errors
///
/// Returns an error when the parent-provided request or response stream cannot
/// be read or written.
pub(crate) fn run_helper() -> Result<()> {
    let mut request = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut request)
        .context("read statvfs helper request")?;

    let stdout = std::io::stdout();
    let mut response = stdout.lock();
    for encoded_path in request
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(encoded_path).context("decode statvfs helper path")?;
        write_record(&mut response, statvfs(path)).context("write statvfs helper response")?;
        response.flush().context("flush statvfs helper response")?;
    }
    Ok(())
}

/// Collect capacity for the explicit local-filesystem allowlist.
///
/// The returned vector always has one slot per input entry. Skipped,
/// unavailable, and unfinished capacities remain `None`.
pub(crate) fn collect(entries: &[MountEntry]) -> Vec<Option<FsSpace>> {
    let started = Instant::now();
    let deadline = started + CAPACITY_DEADLINE;
    let eligible: Vec<(usize, &str)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_local_filesystem(&entry.fstype))
        .map(|(index, entry)| (index, entry.mount_point.as_str()))
        .collect();
    let mut capacities = vec![None; entries.len()];
    let result = collect_eligible(&eligible, &mut capacities, deadline);
    if let Err(error) = result {
        log_event(
            LogLevel::Error,
            "filesystem_capacity_failure",
            &[
                field("eligible", eligible.len()),
                field("error", format!("{error:#}")),
            ],
        );
    }
    capacities
}

fn collect_eligible(
    eligible: &[(usize, &str)],
    capacities: &mut [Option<FsSpace>],
    deadline: Instant,
) -> Result<()> {
    let mut helper_slot = match PENDING_HELPER.try_lock() {
        Ok(slot) => slot,
        Err(TryLockError::WouldBlock) => {
            anyhow::bail!("another statvfs capacity pass is already active")
        }
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
    };
    let previous_reaped = match helper_slot.as_mut() {
        Some(pending) => match pending.try_wait() {
            Ok(Some(_status)) => true,
            Ok(None) => anyhow::bail!("the previous statvfs helper is still terminating"),
            Err(error) => anyhow::bail!("reap the previous statvfs helper: {error}"),
        },
        None => false,
    };
    if previous_reaped {
        *helper_slot = None;
    }
    if eligible.is_empty() {
        return Ok(());
    }

    let mut request = File::from(
        memfd_create("kronika-statvfs-request", MemfdFlags::CLOEXEC)
            .context("create statvfs request buffer")?,
    );
    for (_, path) in eligible {
        request
            .write_all(path.as_bytes())
            .and_then(|()| request.write_all(&[0]))
            .context("encode statvfs request")?;
    }
    request
        .seek(SeekFrom::Start(0))
        .context("rewind statvfs request")?;

    let response = File::from(
        memfd_create("kronika-statvfs-response", MemfdFlags::CLOEXEC)
            .context("create statvfs response buffer")?,
    );
    let response_for_child = response
        .try_clone()
        .context("clone statvfs response buffer")?;
    let executable = std::env::current_exe().context("locate collector for statvfs helper")?;
    let child = Command::new(executable)
        .arg(HELPER_ARGUMENT)
        .stdin(Stdio::from(request))
        .stdout(Stdio::from(response_for_child))
        .stderr(Stdio::null())
        .spawn()
        .context("start statvfs helper")?;

    let (outcome, pending) = poll_helper(child, deadline);
    *helper_slot = pending;
    drop(helper_slot);
    let response_bytes =
        read_complete_records(&response, eligible.len()).context("read statvfs helper response")?;
    let completed = decode_records(&response_bytes, eligible, capacities);
    log_helper_outcome(outcome, eligible.len(), completed);
    Ok(())
}

fn poll_helper(mut child: Child, deadline: Instant) -> (HelperOutcome, Option<Child>) {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let (kill_error, reap_error, reaped, pending) = terminate_helper(child);
            return (
                HelperOutcome::TimedOut {
                    kill_error,
                    reap_error,
                    reaped,
                },
                pending,
            );
        }

        match child.try_wait() {
            Ok(Some(status)) => return (HelperOutcome::Finished(status), None),
            Ok(None) => {}
            Err(error) => {
                let (kill_error, reap_error, reaped, pending) = terminate_helper(child);
                return (
                    HelperOutcome::PollFailed {
                        error: error.to_string(),
                        kill_error,
                        reap_error,
                        reaped,
                    },
                    pending,
                );
            }
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    }
}

fn terminate_helper(mut child: Child) -> (Option<String>, Option<String>, bool, Option<Child>) {
    let kill_error = child.kill().err().map(|error| error.to_string());
    match child.try_wait() {
        Ok(Some(_status)) => (kill_error, None, true, None),
        Ok(None) => (kill_error, None, false, Some(child)),
        Err(error) => (kill_error, Some(error.to_string()), false, Some(child)),
    }
}

fn read_complete_records(response: &File, expected: usize) -> std::io::Result<Vec<u8>> {
    let available_records = usize::try_from(response.metadata()?.len() / RECORD_LEN as u64)
        .unwrap_or(expected)
        .min(expected);
    let mut bytes = vec![0; available_records * RECORD_LEN];
    response.read_exact_at(&mut bytes, 0)?;
    Ok(bytes)
}

fn decode_records(
    response: &[u8],
    eligible: &[(usize, &str)],
    capacities: &mut [Option<FsSpace>],
) -> usize {
    let mut completed = 0;
    for ((index, _), record) in eligible.iter().zip(response.chunks_exact(RECORD_LEN)) {
        completed += 1;
        if record[0] != 1 {
            continue;
        }
        let mut total = [0; 8];
        total.copy_from_slice(&record[1..9]);
        let mut free = [0; 8];
        free.copy_from_slice(&record[9..17]);
        let mut total_inodes = [0; 8];
        total_inodes.copy_from_slice(&record[17..25]);
        let mut available_inodes = [0; 8];
        available_inodes.copy_from_slice(&record[25..33]);
        capacities[*index] = Some(FsSpace {
            total_bytes: i64::from_le_bytes(total),
            free_bytes: i64::from_le_bytes(free),
            total_inodes: i64::from_le_bytes(total_inodes),
            available_inodes: i64::from_le_bytes(available_inodes),
        });
    }
    completed
}

fn write_record(writer: &mut impl Write, space: Option<FsSpace>) -> std::io::Result<()> {
    let mut record = [0; RECORD_LEN];
    if let Some(space) = space {
        record[0] = 1;
        record[1..9].copy_from_slice(&space.total_bytes.to_le_bytes());
        record[9..17].copy_from_slice(&space.free_bytes.to_le_bytes());
        record[17..25].copy_from_slice(&space.total_inodes.to_le_bytes());
        record[25..33].copy_from_slice(&space.available_inodes.to_le_bytes());
    }
    writer.write_all(&record)
}

fn log_helper_outcome(outcome: HelperOutcome, eligible: usize, completed: usize) {
    match outcome {
        HelperOutcome::Finished(status) if status.success() => {}
        HelperOutcome::Finished(status) => log_event(
            LogLevel::Error,
            "filesystem_capacity_failure",
            &[
                field("eligible", eligible),
                field("completed", completed),
                field("status", &status),
            ],
        ),
        HelperOutcome::TimedOut {
            kill_error,
            reap_error,
            reaped,
        } => log_event(
            LogLevel::Error,
            "filesystem_capacity_timeout",
            &[
                field("deadline_ms", CAPACITY_DEADLINE.as_millis()),
                field("eligible", eligible),
                field("completed", completed),
                field("helper_reaped", reaped),
                field("kill_error", kill_error.unwrap_or_default()),
                field("reap_error", reap_error.unwrap_or_default()),
            ],
        ),
        HelperOutcome::PollFailed {
            error,
            kill_error,
            reap_error,
            reaped,
        } => log_event(
            LogLevel::Error,
            "filesystem_capacity_failure",
            &[
                field("eligible", eligible),
                field("completed", completed),
                field("error", error),
                field("helper_reaped", reaped),
                field("kill_error", kill_error.unwrap_or_default()),
                field("reap_error", reap_error.unwrap_or_default()),
            ],
        ),
    }
}

fn is_local_filesystem(fstype: &str) -> bool {
    matches!(
        fstype,
        "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "f2fs" | "zfs" | "tmpfs" | "overlay"
    )
}

#[cfg(test)]
mod tests;
