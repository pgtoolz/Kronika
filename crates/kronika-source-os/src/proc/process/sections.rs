//! Conversion to registry process rows.

use kronika_registry::os_process::OsProcess;
use kronika_registry::os_process_status::OsProcessStatus;
use kronika_registry::{StrId, Ts};

use super::model::{ProcIo, ProcessHotRow, ProcessStatusRow};

/// Convert the hot row to the registry row after interning strings.
#[must_use]
pub fn to_hot_section(
    row: &ProcessHotRow,
    scope: u8,
    comm: StrId,
    cmdline: Option<StrId>,
) -> OsProcess {
    OsProcess {
        ts: Ts(row.ts),
        pid: row.pid,
        starttime: Ts(row.starttime),
        ppid: row.ppid,
        uid: row.uid,
        euid: row.euid,
        gid: row.gid,
        egid: row.egid,
        state: row.state,
        num_threads: row.num_threads,
        tty: row.tty,
        comm,
        cmdline,
        utime: row.utime,
        stime: row.stime,
        nice: row.nice,
        prio: row.prio,
        rtprio: row.rtprio,
        policy: row.policy,
        curcpu: row.curcpu,
        rundelay_ns: row.rundelay_ns,
        blkdelay_ticks: row.blkdelay_ticks,
        nvcsw: row.nvcsw,
        nivcsw: row.nivcsw,
        minflt: row.minflt,
        majflt: row.majflt,
        vmem_kb: row.vmem_kb,
        rmem_kb: row.rmem_kb,
        vswap_kb: row.vswap_kb,
        syscr: row.io.map(|io| io.syscr),
        syscw: row.io.map(|io| io.syscw),
        rchar: row.io.map(|io| io.rchar),
        wchar: row.io.map(|io| io.wchar),
        read_bytes: row.io.map(|io| io.read_bytes),
        write_bytes: row.io.map(|io| io.write_bytes),
        cancelled_write_bytes: row.io.map(|io| io.cancelled_write_bytes),
        exit_signal: row.exit_signal,
        scope,
    }
}

/// Fill the seven nullable I/O counters after their credential-grouped read.
pub const fn set_hot_section_io(row: &mut OsProcess, io: ProcIo) {
    row.syscr = Some(io.syscr);
    row.syscw = Some(io.syscw);
    row.rchar = Some(io.rchar);
    row.wchar = Some(io.wchar);
    row.read_bytes = Some(io.read_bytes);
    row.write_bytes = Some(io.write_bytes);
    row.cancelled_write_bytes = Some(io.cancelled_write_bytes);
}

/// Convert the extended status row to the registry row.
#[must_use]
pub const fn to_status_section(row: &ProcessStatusRow, scope: u8) -> OsProcessStatus {
    OsProcessStatus {
        ts: Ts(row.ts),
        pid: row.pid,
        starttime: Ts(row.starttime),
        vm_data: row.status.vm_data,
        vm_stk: row.status.vm_stk,
        vm_lib: row.status.vm_lib,
        vm_lck: row.status.vm_lck,
        vm_pte: row.status.vm_pte,
        vm_peak: row.status.vm_peak,
        vm_hwm: row.status.vm_hwm,
        threads: row.status.threads,
        fdsize: row.status.fdsize,
        voluntary_ctxt_switches: row.status.voluntary_ctxt_switches,
        nonvoluntary_ctxt_switches: row.status.nonvoluntary_ctxt_switches,
        scope,
    }
}
