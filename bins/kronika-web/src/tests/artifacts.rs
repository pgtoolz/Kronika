use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use flate2::read::GzDecoder;
use http_body_util::BodyExt as _;
use hyper::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, ETAG, HeaderValue, VARY};
use hyper::{HeaderMap, StatusCode};
use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::InstanceMetadata;
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::os_netdev::OsNetdev;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::os_user::OsUser;
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::pg_log::{PgLogErrors, PgLogTempFiles};
use kronika_registry::pg_settings::PgSettings;
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::pg_stat_database::PgStatDatabaseV1;
use kronika_registry::pg_stat_progress_vacuum::PgStatProgressVacuumV1;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::pg_stat_user_indexes::{PgStatUserIndexesV1, PgStatUserIndexesV2};
use kronika_registry::pg_stat_user_tables::PgStatUserTablesV1;
use kronika_registry::pg_store_plans::{PgStorePlansOsscV1, PgStorePlansVadvV1};
use kronika_registry::pgbouncer_events::PgBouncerEvents;
use kronika_registry::{Section, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::Value;

use crate::api::{ApiError, CachePolicy, Prepared, ResponseMeta};
use crate::config::SOURCE_OS;
use crate::encoding::AcceptedEncodings;

mod heatmap_rss;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SOURCES: u32 = 0b11;

pub(crate) type NamedIndexSnapshot<'a> = (
    i64,
    u32,
    u32,
    i64,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
);

type DmlTableSnapshot<'a> = (i64, u32, u32, [i64; 4], &'a str, &'a str, &'a str);

pub(crate) type PlacedTableSnapshot<'a> = (
    i64,
    u32,
    u32,
    i64,
    &'a str,
    &'a str,
    &'a str,
    Option<u32>,
    Option<&'a str>,
    i64,
    Option<i64>,
);

type PlacedIndexSnapshot<'a> = (
    i64,
    u32,
    u32,
    i64,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    u32,
    Option<&'a str>,
    i64,
);

pub(crate) struct Fixture {
    directory: tempfile::TempDir,
    writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary data root");
        let root = DataRoot::open(directory.path()).expect("open data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            writer,
            journal,
            address,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        self.directory.path()
    }

    fn position(&self) -> u64 {
        u64::try_from(self.journal.bytes()).expect("journal length fits u64")
    }

    fn append_diskstats(&mut self, rows: &[(i64, i32, i64)]) {
        let mut buffers = SectionBuffers::new();
        for &(ts, minor, reads) in rows {
            buffers
                .push(diskstats(ts, minor, reads))
                .expect("diskstats row fits");
        }
        self.append(buffers);
    }

    fn append_resolved_diskstats(&mut self, rows: &[(i64, i32, i64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let device = StrId(
            interner
                .intern(b"fixture-device")
                .expect("intern fixture device")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(ts, minor, reads) in rows {
            buffers
                .push(diskstats_with_device(ts, minor, reads, device))
                .expect("diskstats row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode fixture device dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode diskstats fixture")
            .expect("nonempty diskstats fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append diskstats fixture");
    }

    fn append_named_diskstats(&mut self, rows: &[(i32, &str)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(minor, device) in rows {
            let device = StrId(
                interner
                    .intern(device.as_bytes())
                    .expect("intern device")
                    .get(),
            );
            buffers
                .push(diskstats_with_device(100, minor, 1, device))
                .expect("named diskstats row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode device dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode named diskstats fixture")
            .expect("nonempty named diskstats fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append named diskstats fixture");
    }

    fn append_large_cgroup_cpu(&mut self, rows: usize, selected: usize) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for index in 0..rows {
            let path = if index == selected {
                "/collector".to_owned()
            } else {
                format!("/tree/cgroup-{index}")
            };
            let cgroup_path = StrId(
                interner
                    .intern(path.as_bytes())
                    .expect("intern large cgroup path")
                    .get(),
            );
            buffers
                .push(OsCgroupCpu {
                    ts: Ts(200),
                    cgroup_path,
                    usage_usec: i64::try_from(index).expect("fixture index fits i64"),
                    user_usec: 0,
                    system_usec: 0,
                    throttled_usec: 0,
                    nr_throttled: 0,
                    quota_usec: -1,
                    period_usec: 100_000,
                    scope: 3,
                })
                .expect("large cgroup CPU row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode large cgroup dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode large cgroup CPU fixture")
            .expect("nonempty large cgroup CPU fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append large cgroup CPU fixture");
    }

    fn append_blob_diskstats(&mut self, bytes: &[u8], reads: i64) {
        let mut interner =
            Interner::new(DictLimits::new(8, 16).expect("fixture dictionary limits"));
        let device = StrId(interner.intern(bytes).expect("intern blob").get());
        let dictionary = dict::encode(interner.window()).expect("encode blob dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(diskstats_with_device(100, 0, reads, device))
            .expect("diskstats row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode blob fixture part")
            .expect("nonempty blob fixture part");
        self.journal
            .append(self.address.id, &part)
            .expect("append blob fixture part");
    }

    fn append_named_then_unreadable_diskstats(&mut self, valid_rows: i32) {
        let mut interner = Interner::new(DictLimits::default());
        let device = StrId(interner.intern(b"nvme0n1").expect("intern device").get());
        let dictionary = dict::encode(interner.window()).expect("encode device dictionary");
        let mut buffers = SectionBuffers::new();
        for minor in 0..valid_rows {
            buffers
                .push(diskstats_with_device(100, minor, i64::from(minor), device))
                .expect("valid row fits");
        }
        buffers
            .push(diskstats_with_device(
                100,
                valid_rows,
                i64::from(valid_rows),
                StrId(999_999),
            ))
            .expect("unreadable row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode unreadable fixture")
            .expect("nonempty unreadable fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append unreadable fixture");
    }

    fn append_ranked_diskstats_with_unreadable_loser(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let device = StrId(interner.intern(b"nvme0n1").expect("intern device").get());
        let dictionary = dict::encode(interner.window()).expect("encode device dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(diskstats_with_device(100, -1, 1, StrId(999_999)))
            .expect("unreadable row fits");
        buffers
            .push(diskstats_with_device(100, 2, 2, device))
            .expect("ranked row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked fixture")
            .expect("nonempty ranked fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked fixture");
    }

    fn append_health(&mut self) {
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(100),
                hostname: StrId(901),
                kernel_version: StrId(902),
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: StrId(903),
                btime: Ts(1),
                postgresql_enabled: false,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: None,
            })
            .expect("metadata row fits");
        for row in [
            psi(100, 0, 0),
            psi(100, 1, 0),
            psi(100, 2, 0),
            psi(200, 0, 50),
            psi(200, 1, 20),
            psi(200, 2, 0),
        ] {
            buffers.push(row).expect("psi row fits");
        }
        self.append(buffers);
    }

    pub(crate) fn append_postgres_health(&mut self, active: u32) {
        self.append_postgres_health_at(100, active);
    }

    fn append_postgres_health_at(&mut self, at: i64, active: u32) {
        self.append_postgres_health_with_interval(at, active, 30);
    }

    pub(crate) fn append_postgres_health_with_interval(
        &mut self,
        at: i64,
        active: u32,
        interval_seconds: u64,
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let active_state = StrId(interner.intern(b"active").expect("active state").get());
        let idle_state = StrId(interner.intern(b"idle").expect("idle state").get());
        let query = StrId(
            interner
                .intern(b"QUERY-TEXT-MUST-STAY-OUT-OF-IDX")
                .expect("query text")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("health dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(at),
                hostname: StrId(901),
                kernel_version: StrId(902),
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: StrId(903),
                btime: Ts(1),
                postgresql_enabled: true,
                postgresql_interval_seconds: interval_seconds,
                postgresql_effective_cpus: Some(2),
            })
            .expect("metadata row fits");
        for row in [
            psi(at, 0, 0),
            psi(at, 1, 0),
            psi(at, 2, 0),
            psi(at + 100, 0, 50),
            psi(at + 100, 1, 20),
            psi(at + 100, 2, 0),
        ] {
            buffers.push(row).expect("psi row fits");
        }
        for pid in 0..active {
            buffers
                .push(activity(
                    at + 50,
                    i32::try_from(pid).expect("fixture pid"),
                    active_state,
                    query,
                ))
                .expect("active row fits");
        }
        buffers
            .push(activity(at + 50, 10_000, idle_state, query))
            .expect("idle row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode health fixture")
            .expect("nonempty health fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append health fixture");
    }

    fn append_finding_rows(
        &mut self,
        process_rows: &[(i64, Option<i64>)],
        statement_rows: &[(i64, i64, f64)],
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(interner.intern(b"fixture").expect("intern label").get());
        let dictionary = dict::encode(interner.window()).expect("finding dictionary");
        let mut buffers = SectionBuffers::new();
        for &(ts, read_bytes) in process_rows {
            buffers
                .push(process(ts, read_bytes, label))
                .expect("process row fits");
        }
        for &(ts, calls, total_exec_time) in statement_rows {
            buffers
                .push(statement(ts, calls, total_exec_time, label))
                .expect("statement row fits");
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode finding rows")
            .expect("nonempty finding rows");
        self.journal
            .append(self.address.id, &part)
            .expect("append finding rows");
    }

    /// `rows` is `(ts, pid, rmem_kb, comm)`. Multiple samples let gauge window
    /// maxima differ from last column values; `comm` is the grouping key.
    pub(crate) fn append_process_gauge_rows(&mut self, rows: &[(i64, i32, i64, &str)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(ts, pid, rmem_kb, comm) in rows {
            let label = StrId(
                interner
                    .intern(comm.as_bytes())
                    .expect("intern process comm")
                    .get(),
            );
            let mut row = process(ts, None, label);
            row.pid = pid;
            row.starttime = Ts(SEGMENT_ID - 1_000_000 + i64::from(pid));
            row.rmem_kb = rmem_kb;
            buffers.push(row).expect("process gauge row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode process gauge dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode process gauge fixture")
            .expect("nonempty process gauge fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append process gauge fixture");
    }

    pub(crate) fn append_process_cmdline(&mut self, ts: i64, pid: i32, cmdline: &str) {
        let mut interner = Interner::new(DictLimits::default());
        let comm = StrId(interner.intern(b"fixture").expect("intern comm").get());
        let cmdline = StrId(
            interner
                .intern(cmdline.as_bytes())
                .expect("intern command line")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("encode process dictionary");
        let mut row = process(ts, None, comm);
        row.pid = pid;
        row.cmdline = Some(cmdline);
        let mut buffers = SectionBuffers::new();
        buffers.push(row).expect("process row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode process row")
            .expect("nonempty process row");
        self.journal
            .append(self.address.id, &part)
            .expect("append process row");
    }

    pub(crate) fn append_process_counter_rows(&mut self, rows: &[(i64, Option<i64>)]) {
        self.append_finding_rows(rows, &[]);
    }

    fn append_process_summary_snapshot(
        &mut self,
        ts: i64,
        counter: i64,
        changed_starttime_pid: Option<i32>,
        activity_ts: i64,
        activity_pids: std::ops::Range<i32>,
        future_activity: Option<(i64, std::ops::Range<i32>)>,
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(
            interner
                .intern(b"fixture")
                .expect("intern process label")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("process summary dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(ts),
                hostname: label,
                kernel_version: label,
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: label,
                btime: Ts(1),
                postgresql_enabled: true,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: Some(2),
            })
            .expect("summary metadata fits");
        for pid in 0..205 {
            let mut row = process_for_summary(ts, pid, counter, label);
            if changed_starttime_pid == Some(pid) {
                row.starttime = Ts(row.starttime.0 + 1);
            }
            buffers.push(row).expect("summary process row fits");
        }
        for pid in activity_pids {
            buffers
                .push(activity(activity_ts, pid, label, label))
                .expect("past activity row fits");
        }
        if let Some((future_ts, pids)) = future_activity {
            for pid in pids {
                buffers
                    .push(activity(future_ts, pid, label, label))
                    .expect("future activity row fits");
            }
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode process summary snapshot")
            .expect("nonempty process summary snapshot");
        self.journal
            .append(self.address.id, &part)
            .expect("append process summary snapshot");
    }

    /// `processes` is `(pid, uid, euid, utime, stime)`; CPU fields populate
    /// `cpu_time_ticks`.
    fn append_user_processes(
        &mut self,
        ts: i64,
        processes: &[(i32, u32, u32, i64, i64)],
        names: &[(u32, &str)],
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let command = StrId(
            interner
                .intern(b"fixture-worker")
                .expect("intern command")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(pid, uid, euid, utime, stime) in processes {
            let mut row = process(ts, None, command);
            row.pid = pid;
            row.starttime = Ts(SEGMENT_ID - 1_000_000 + i64::from(pid));
            row.uid = uid;
            row.euid = euid;
            row.utime = utime;
            row.stime = stime;
            buffers.push(row).expect("process row fits");
        }
        for &(uid, name) in names {
            let username = StrId(
                interner
                    .intern(name.as_bytes())
                    .expect("intern user name")
                    .get(),
            );
            buffers
                .push(OsUser {
                    ts: Ts(ts),
                    uid,
                    username,
                    scope: 0,
                })
                .expect("user row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode process user dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode process user fixture")
            .expect("nonempty process user fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append process user fixture");
    }

    fn append_quantitative_processes(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let label = fixture_label(&mut interner, "quantity-worker");
        let dictionary = dict::encode(interner.window()).expect("process quantity dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(2_000_000),
                hostname: label,
                kernel_version: label,
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: label,
                btime: Ts(1),
                postgresql_enabled: true,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: Some(2),
            })
            .expect("process quantity metadata fits");
        let large = 1_i64 << 53;
        for (ts, pid, starttime, ticks, bytes, rss_kib) in [
            (1_000_000, 1, 10, 0, large, 3_072),
            (2_000_000, 1, 10, 20, large + 1_048_576, 3_072),
            (1_000_000, 2, 20, 0, large, 2_048),
            (2_000_000, 2, 20, 10, large + 100, 2_048),
            (1_000_000, 3, 30, 0, large, 4_096),
            (2_000_000, 3, 31, 50, large + 2_097_152, 4_096),
            (1_000_000, 4, 40, 100, 1_000, 8_192),
            (2_000_000, 4, 40, 90, 900, 8_192),
            (2_000_000, 5, 50, 50, large, 8_192),
            (1_000_000, 6, 60, 0, 4_096, 1_024),
            (2_000_000, 6, 60, 0, 4_096, 1_024),
        ] {
            let mut row = process(ts, Some(bytes), label);
            row.pid = pid;
            row.starttime = Ts(starttime);
            row.utime = ticks / 2;
            row.stime = ticks - row.utime;
            row.rmem_kb = rss_kib;
            row.vmem_kb = rss_kib * 2;
            row.vswap_kb = rss_kib / 4;
            row.num_threads = u32::try_from(pid + 1).expect("fixture thread count");
            row.write_bytes = Some(bytes / 2);
            row.minflt = ticks;
            row.majflt = ticks / 2;
            row.nvcsw = ticks;
            row.nivcsw = ticks;
            row.rundelay_ns = ticks * 1_000_000;
            row.blkdelay_ticks = ticks;
            if pid != 6 {
                row.syscr = Some(ticks);
                row.syscw = Some(ticks * 2);
                row.rchar = Some(bytes + 4_096);
                row.wchar = Some(bytes / 2 + 8_192);
            }
            buffers.push(row).expect("process quantity row fits");
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode process quantities")
            .expect("nonempty process quantities");
        self.journal
            .append(self.address.id, &part)
            .expect("append process quantities");
    }

    fn append_statement_universe(&mut self, rows: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for queryid in 0..rows {
            let text = if queryid == 0 {
                "owner blocker-needle outside page one".to_owned()
            } else {
                format!("fixture statement {queryid}")
            };
            let query = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern statement text")
                    .get(),
            );
            let mut row = statement(100, 1, 1.0, query);
            row.queryid = Some(queryid);
            buffers.push(row).expect("statement row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("statement dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode statements")
            .expect("nonempty statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append statements");
    }

    pub(crate) fn append_statement_snapshots(&mut self, rows: &[(i64, i64, i64, f64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(ts, queryid, calls, total_exec_time) in rows {
            let text = format!("boundary statement {queryid}");
            let query = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern boundary statement")
                    .get(),
            );
            let mut row = statement(ts, calls, total_exec_time, query);
            row.queryid = Some(queryid);
            row.datname = Some(StrId(
                interner
                    .intern(b"operators")
                    .expect("intern statement database")
                    .get(),
            ));
            row.usename = Some(StrId(
                interner
                    .intern(b"reporter")
                    .expect("intern statement role")
                    .get(),
            ));
            buffers.push(row).expect("boundary statement row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("boundary statement dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode boundary statements")
            .expect("nonempty boundary statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append boundary statements");
    }

    fn append_statement_text_matches(
        &mut self,
        timestamp: i64,
        query_id: i64,
        texts: &[Option<&str>],
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let unused = fixture_label(&mut interner, "unused statement text");
        let mut buffers = SectionBuffers::new();
        for (index, text) in texts.iter().enumerate() {
            let mut row = statement(timestamp, 1, 1.0, unused);
            row.queryid = Some(query_id);
            row.dbid = 73_u32.saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
            row.query = text.map(|text| fixture_label(&mut interner, text));
            buffers.push(row).expect("statement text row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("statement text dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode statement text matches")
            .expect("nonempty statement text matches");
        self.journal
            .append(self.address.id, &part)
            .expect("append statement text matches");
    }

    fn append_plan_universe(&mut self, rows: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for queryid in 0..rows {
            let text = if queryid == 0 {
                "owner plan-needle outside page one".to_owned()
            } else {
                format!("fixture plan {queryid}")
            };
            let plan_text = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern plan text")
                    .get(),
            );
            buffers
                .push(store_plan(100, queryid, plan_text))
                .expect("plan row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode plans")
            .expect("nonempty plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append plans");
    }

    pub(crate) fn append_plan_snapshots(&mut self, rows: &[(i64, i64, i64, f64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(ts, queryid, calls, total_time) in rows {
            let text = format!("boundary plan {queryid}");
            let plan_text = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern boundary plan")
                    .get(),
            );
            let mut row = store_plan(ts, queryid, plan_text);
            row.calls = calls;
            row.total_time = total_time;
            buffers.push(row).expect("boundary plan row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("boundary plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode boundary plans")
            .expect("nonempty boundary plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append boundary plans");
    }

    /// At `ts=100` all counters are zero; `ts=200` uses the readings below,
    /// producing non-null, hand-computable deltas.
    pub(crate) fn append_ranked_statements(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        let readings = [
            (1, 10, 100.0, 100, 80, 20, 0, 0, 100, 100.0, 10.0, 9.0),
            (2, 2, 30.0, 60, 9, 1, 40, 0, 60, 40.0, 1.0, 2.0),
            (3, 1, 5.0, 1, 1, 2, 0, 0, 5, 1.0, 5.0, 1.0),
        ];
        for (
            queryid,
            calls,
            execution,
            rows,
            hit,
            read,
            local_hit,
            local_read,
            wal,
            planning,
            mean,
            deviation,
        ) in readings
        {
            let text = format!("ranked statement {queryid}");
            let query = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern ranked statement")
                    .get(),
            );
            for ts in [100, 200] {
                let current = ts == 200;
                let mut row = statement(
                    ts,
                    if current { calls } else { 0 },
                    if current { execution } else { 0.0 },
                    query,
                );
                row.queryid = Some(queryid);
                row.rows = if current { rows } else { 0 };
                row.shared_blks_hit = if current { hit } else { 0 };
                row.shared_blks_read = if current { read } else { 0 };
                row.local_blks_hit = if current { local_hit } else { 0 };
                row.local_blks_read = if current { local_read } else { 0 };
                row.wal_bytes = if current { wal } else { 0 };
                row.total_plan_time = if current { planning } else { 0.0 };
                row.mean_exec_time = mean;
                row.stddev_exec_time = deviation;
                buffers.push(row).expect("ranked statement row fits");
            }
        }
        let dictionary = dict::encode(interner.window()).expect("ranked statement dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked statements")
            .expect("nonempty ranked statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked statements");
    }

    /// Plan-layout counterpart to `append_ranked_statements`, with zero
    /// predecessors at `ts=100` and current values at `ts=200`.
    pub(crate) fn append_ranked_plans(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        let readings = [
            (1, 10, 100.0, 100, 80, 20, 0, 0),
            (2, 2, 30.0, 60, 9, 1, 40, 0),
            (3, 1, 5.0, 1, 1, 2, 0, 0),
        ];
        for (queryid, calls, execution, rows, hit, read, local_hit, local_read) in readings {
            let text = format!("ranked plan {queryid}");
            let plan_text = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern ranked plan")
                    .get(),
            );
            for ts in [100, 200] {
                let current = ts == 200;
                let mut row = store_plan(ts, queryid, plan_text);
                row.calls = if current { calls } else { 0 };
                row.total_time = if current { execution } else { 0.0 };
                row.rows = if current { rows } else { 0 };
                row.shared_blks_hit = if current { hit } else { 0 };
                row.shared_blks_read = if current { read } else { 0 };
                row.local_blks_hit = if current { local_hit } else { 0 };
                row.local_blks_read = if current { local_read } else { 0 };
                buffers.push(row).expect("ranked plan row fits");
            }
        }
        let dictionary = dict::encode(interner.window()).expect("ranked plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked plans")
            .expect("nonempty ranked plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked plans");
    }

    /// Vadv plan-layout fixture (`type_id` `1_004_001`), whose
    /// `total_plan_time` makes `derived_plan_time_fraction` non-null.
    pub(crate) fn append_ranked_vadv_plans(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let label = fixture_label(&mut interner, "ranked vadv plan");
        let mut buffers = SectionBuffers::new();
        for ts in [100, 200] {
            let current = ts == 200;
            let mut row = store_plan_vadv(ts, label);
            row.calls = if current { 10 } else { 0 };
            row.total_time = if current { 100.0 } else { 0.0 };
            row.total_plan_time = if current { 25.0 } else { 0.0 };
            buffers.push(row).expect("ranked vadv plan row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("ranked vadv plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked vadv plans")
            .expect("nonempty ranked vadv plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked vadv plans");
    }

    fn append_postgres_summary_relations(&mut self) {
        let mut buffers = SectionBuffers::new();
        for (ts, commits, rollbacks) in [(100, 100, 10), (200, 180, 30)] {
            buffers
                .push(postgres_database(ts, commits, rollbacks, 0))
                .expect("PostgreSQL database summary row fits");
        }
        for (ts, seq_scan, idx_scan) in [(100, 10, 10), (200, 30, 40)] {
            let mut row = user_table(ts, 73, 81, seq_scan);
            row.idx_scan = Some(idx_scan);
            buffers
                .push(row)
                .expect("PostgreSQL table summary row fits");
        }
        for (indexrelid, current_scans) in [(91, 4), (92, 0)] {
            for (ts, scans) in [(100, 0), (200, current_scans)] {
                buffers
                    .push(user_index_v1(ts, 73, indexrelid, scans))
                    .expect("PostgreSQL index summary row fits");
            }
        }
        self.append(buffers);
    }

    /// `rows` is `(ts, xact_commit, xact_rollback, deadlocks)`, one
    /// `pg_stat_database` row per timestamp for database 73.
    fn append_postgres_database_rows(&mut self, rows: &[(i64, i64, i64, i64)]) {
        let mut buffers = SectionBuffers::new();
        for &(ts, xact_commit, xact_rollback, deadlocks) in rows {
            buffers
                .push(postgres_database(ts, xact_commit, xact_rollback, deadlocks))
                .expect("PostgreSQL database row fits");
        }
        self.append(buffers);
    }

    /// Like `append_postgres_database_rows`, but interns `datname`; full-field
    /// MCP projections require it.
    pub(crate) fn append_postgres_database_snapshots(&mut self, rows: &[(i64, i64, i64, i64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let datname = StrId(interner.intern(b"db").expect("intern datname").get());
        let mut buffers = SectionBuffers::new();
        for &(ts, xact_commit, xact_rollback, deadlocks) in rows {
            let mut row = postgres_database(ts, xact_commit, xact_rollback, deadlocks);
            row.datname = Some(datname);
            buffers.push(row).expect("PostgreSQL database row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode database dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode database fixture")
            .expect("nonempty database fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append database fixture");
    }

    /// `rows` is `(ts, pid, state, lock_mode)`. `state` also supplies the
    /// application, client, and backend-type fixture columns.
    pub(crate) fn append_postgres_lock_rows(&mut self, rows: &[(i64, i32, &str, &str)]) {
        let mut interner = Interner::new(DictLimits::default());
        let datname = StrId(interner.intern(b"db").expect("intern datname").get());
        let usename = StrId(interner.intern(b"dba").expect("intern usename").get());
        let query = StrId(interner.intern(b"SELECT 1").expect("intern query").get());
        let lock_locktype = StrId(
            interner
                .intern(b"relation")
                .expect("intern lock_locktype")
                .get(),
        );
        let lock_relname = StrId(
            interner
                .intern(b"alpha")
                .expect("intern lock_relname")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(ts, pid, state, lock_mode) in rows {
            let state_id = StrId(
                interner
                    .intern(state.as_bytes())
                    .expect("intern lock state")
                    .get(),
            );
            let lock_mode_id = StrId(
                interner
                    .intern(lock_mode.as_bytes())
                    .expect("intern lock_mode")
                    .get(),
            );
            buffers
                .push(PgLocksV2 {
                    ts: Ts(ts),
                    pid,
                    blocked_by: vec![],
                    datid: 16_384,
                    datname,
                    usename: Some(usename),
                    application_name: state_id,
                    client_addr: state_id,
                    backend_type: state_id,
                    state: Some(state_id),
                    wait_event_type: None,
                    wait_event: None,
                    query,
                    backend_xid_age: None,
                    backend_xmin_age: None,
                    backend_start: Some(Ts(ts)),
                    xact_start: None,
                    query_start: None,
                    state_change: None,
                    lock_locktype: Some(lock_locktype),
                    lock_mode: Some(lock_mode_id),
                    lock_database: Some(16_384),
                    lock_relation: Some(12_345),
                    lock_relname: Some(lock_relname),
                    lock_page: None,
                    lock_tuple: None,
                    lock_virtualxid: None,
                    lock_transactionid: None,
                    lock_classid: None,
                    lock_objid: None,
                    lock_objsubid: None,
                    lock_target: None,
                    waitstart: None,
                })
                .expect("lock row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode lock dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode lock fixture")
            .expect("nonempty lock fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append lock fixture");
    }

    /// `rows` is `(ts, pid, phase, heap_blks_total, heap_blks_scanned,
    /// heap_blks_vacuumed)`: one backend running `VACUUM` per entry, all on
    /// the same fixed relation.
    pub(crate) fn append_postgres_vacuum_rows(&mut self, rows: &[(i64, i32, &str, i64, i64, i64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let datname = StrId(interner.intern(b"db").expect("intern datname").get());
        let schemaname = StrId(interner.intern(b"public").expect("intern schemaname").get());
        let relname = StrId(interner.intern(b"alpha").expect("intern relname").get());
        let mut buffers = SectionBuffers::new();
        for &(ts, pid, phase, heap_blks_total, heap_blks_scanned, heap_blks_vacuumed) in rows {
            let phase_id = StrId(
                interner
                    .intern(phase.as_bytes())
                    .expect("intern phase")
                    .get(),
            );
            buffers
                .push(PgStatProgressVacuumV1 {
                    ts: Ts(ts),
                    pid,
                    datid: 16_385,
                    datname,
                    relid: 16_384,
                    schemaname: Some(schemaname),
                    relname: Some(relname),
                    is_autovacuum: false,
                    phase: phase_id,
                    heap_blks_total,
                    heap_blks_scanned,
                    heap_blks_vacuumed,
                    index_vacuum_count: 0,
                    max_dead_tuples: 0,
                    num_dead_tuples: 0,
                })
                .expect("vacuum row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode vacuum dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode vacuum fixture")
            .expect("nonempty vacuum fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append vacuum fixture");
    }

    pub(crate) fn append_instance_facts(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let mut intern = |value: &str| {
            StrId(
                interner
                    .intern(value.as_bytes())
                    .expect("intern instance string")
                    .get(),
            )
        };
        let hostname = intern("fixture-host");
        let kernel_version = intern("6.1.0-fixture");
        let boot_id = intern("boot-fixture");
        let dictionary = dict::encode(interner.window()).expect("instance dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(100),
                hostname,
                kernel_version,
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id,
                btime: Ts(1),
                postgresql_enabled: true,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: Some(2),
            })
            .expect("instance row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode instance row")
            .expect("nonempty instance row");
        self.journal
            .append(self.address.id, &part)
            .expect("append instance row");
    }

    pub(crate) fn append_postgres_block_size(&mut self, block_size: u128) {
        self.append_postgres_setting("block_size", &block_size.to_string(), "default");
    }

    pub(crate) fn append_postgres_setting(&mut self, name: &str, value: &str, source: &str) {
        let mut interner = Interner::new(DictLimits::default());
        let intern = |interner: &mut Interner, value: &str| {
            StrId(
                interner
                    .intern(value.as_bytes())
                    .expect("intern setting")
                    .get(),
            )
        };
        let name = intern(&mut interner, name);
        let setting = intern(&mut interner, value);
        let database = intern(&mut interner, "postgres");
        let role = intern(&mut interner, "collector");
        let source = intern(&mut interner, source);
        let context = intern(&mut interner, "internal");
        let vartype = intern(&mut interner, "integer");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgSettings {
                ts: Ts(200),
                datid: 1,
                datname: database,
                usesysid: 2,
                usename: role,
                name,
                setting,
                unit: None,
                source,
                sourcefile: None,
                sourceline: None,
                pending_restart: false,
                context,
                vartype,
                boot_val: Some(setting),
                reset_val: Some(setting),
            })
            .expect("block-size setting row fits");
        let dictionary = dict::encode(interner.window()).expect("block-size dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode block-size setting")
            .expect("nonempty block-size setting");
        self.journal
            .append(self.address.id, &part)
            .expect("append block-size setting");
    }

    pub(crate) fn append_postgres_settings(&mut self, count: usize, value: &str, source: &str) {
        let mut interner = Interner::new(DictLimits::default());
        let intern = |interner: &mut Interner, value: &str| {
            StrId(
                interner
                    .intern(value.as_bytes())
                    .expect("intern bulk setting")
                    .get(),
            )
        };
        let setting = intern(&mut interner, value);
        let database = intern(&mut interner, "postgres");
        let role = intern(&mut interner, "collector");
        let source = intern(&mut interner, source);
        let context = intern(&mut interner, "user");
        let vartype = intern(&mut interner, "string");
        let mut buffers = SectionBuffers::new();
        for index in 0..count {
            let name = intern(&mut interner, &format!("fixture_setting_{index}"));
            buffers
                .push(PgSettings {
                    ts: Ts(200),
                    datid: 1,
                    datname: database,
                    usesysid: 2,
                    usename: role,
                    name,
                    setting,
                    unit: None,
                    source,
                    sourcefile: None,
                    sourceline: None,
                    pending_restart: false,
                    context,
                    vartype,
                    boot_val: Some(setting),
                    reset_val: Some(setting),
                })
                .expect("bulk setting row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("bulk settings dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode bulk settings")
            .expect("nonempty bulk settings");
        self.journal
            .append(self.address.id, &part)
            .expect("append bulk settings");
    }

    fn append_vadv_plan_quantities(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let label = fixture_label(&mut interner, "vadv-quantity-plan");
        let mut buffers = SectionBuffers::new();
        for ts in [100, 200] {
            let current = ts == 200;
            let mut row = store_plan_vadv(ts, label);
            row.calls = if current { 10 } else { 0 };
            row.slow_log_calls = if current { 4 } else { 0 };
            row.total_time = if current { 75.0 } else { 0.0 };
            row.total_plan_time = if current { 25.0 } else { 0.0 };
            buffers.push(row).expect("vadv quantity plan fits");
        }
        let dictionary = dict::encode(interner.window()).expect("vadv quantity dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode vadv quantities")
            .expect("nonempty vadv quantities");
        self.journal
            .append(self.address.id, &part)
            .expect("append vadv quantities");
    }

    fn append_relation_snapshots(
        &mut self,
        tables: &[(i64, u32, u32, i64)],
        indexes_v1: &[(i64, u32, u32, i64)],
        indexes_v2: &[(i64, u32, u32, i64)],
    ) {
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, relid, seq_scan) in tables {
            buffers
                .push(user_table(ts, datid, relid, seq_scan))
                .expect("table snapshot row fits");
        }
        for &(ts, datid, indexrelid, idx_scan) in indexes_v1 {
            buffers
                .push(user_index_v1(ts, datid, indexrelid, idx_scan))
                .expect("V1 index snapshot row fits");
        }
        for &(ts, datid, indexrelid, idx_scan) in indexes_v2 {
            buffers
                .push(user_index_v2(ts, datid, indexrelid, idx_scan))
                .expect("V2 index snapshot row fits");
        }
        self.append(buffers);
    }

    pub(crate) fn append_named_table_snapshots(
        &mut self,
        rows: &[(i64, u32, u32, i64, &str, &str, &str)],
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let tablespace = StrId(
            interner
                .intern(b"pg_default")
                .expect("intern tablespace")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, relid, seq_scan, datname, schemaname, relname) in rows {
            let mut row = user_table(ts, datid, relid, seq_scan);
            row.datname = StrId(
                interner
                    .intern(datname.as_bytes())
                    .expect("intern database")
                    .get(),
            );
            row.schemaname = StrId(
                interner
                    .intern(schemaname.as_bytes())
                    .expect("intern schema")
                    .get(),
            );
            row.relname = StrId(
                interner
                    .intern(relname.as_bytes())
                    .expect("intern table")
                    .get(),
            );
            row.tablespace = Some(tablespace);
            buffers.push(row).expect("named table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode relation dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode named table snapshots")
            .expect("nonempty named table snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append named table snapshots");
    }

    fn append_large_named_table_snapshots(&mut self, objects: u32) {
        let mut interner = Interner::new(DictLimits::default());
        let database = fixture_label(&mut interner, "fixture_db");
        let schema = fixture_label(&mut interner, "public");
        let tablespace = fixture_label(&mut interner, "pg_default");
        let mut buffers = SectionBuffers::new();
        for relid in 0..objects {
            let relation = fixture_label(&mut interner, &format!("relation_{relid:05}"));
            for (timestamp, seq_scan) in [(100, i64::from(relid)), (200, i64::from(relid) + 10)] {
                let mut row = user_table(timestamp, 1, relid + 1, seq_scan);
                row.datname = database;
                row.schemaname = schema;
                row.relname = relation;
                row.tablespace = Some(tablespace);
                buffers.push(row).expect("large named table row fits");
            }
        }
        let dictionary = dict::encode(interner.window()).expect("large relation dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode large relation fixture")
            .expect("nonempty large relation fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append large relation fixture");
    }

    pub(crate) fn append_placed_table_snapshots(&mut self, rows: &[PlacedTableSnapshot<'_>]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(
            ts,
            datid,
            relid,
            seq_scan,
            datname,
            schema,
            table,
            tablespace_oid,
            tablespace,
            main_fork_bytes,
            toast_bytes,
        ) in rows
        {
            let mut row = user_table(ts, datid, relid, seq_scan);
            row.datname = fixture_label(&mut interner, datname);
            row.schemaname = fixture_label(&mut interner, schema);
            row.relname = fixture_label(&mut interner, table);
            row.tablespace_oid = tablespace_oid;
            row.tablespace = tablespace.map(|label| fixture_label(&mut interner, label));
            row.main_fork_bytes = main_fork_bytes;
            row.toast_bytes = toast_bytes;
            buffers.push(row).expect("placed table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("placed table dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode placed tables")
            .expect("nonempty placed tables");
        self.journal
            .append(self.address.id, &part)
            .expect("append placed tables");
    }

    fn append_dml_table_snapshots(&mut self, rows: &[DmlTableSnapshot<'_>]) {
        let mut interner = Interner::new(DictLimits::default());
        let tablespace = StrId(
            interner
                .intern(b"pg_default")
                .expect("intern tablespace")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, relid, [inserted, updated, deleted, hot], datname, schema, table) in rows {
            let mut row = user_table(ts, datid, relid, 0);
            row.n_tup_ins = inserted;
            row.n_tup_upd = updated;
            row.n_tup_del = deleted;
            row.n_tup_hot_upd = hot;
            row.datname = StrId(
                interner
                    .intern(datname.as_bytes())
                    .expect("intern database")
                    .get(),
            );
            row.schemaname = StrId(
                interner
                    .intern(schema.as_bytes())
                    .expect("intern schema")
                    .get(),
            );
            row.relname = StrId(
                interner
                    .intern(table.as_bytes())
                    .expect("intern table")
                    .get(),
            );
            row.tablespace = Some(tablespace);
            buffers.push(row).expect("DML table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode DML relation dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode DML table snapshots")
            .expect("nonempty DML table snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append DML table snapshots");
    }

    fn append_buffered_table_snapshots(&mut self, rows: &[(i64, u32, u32, [i64; 8])]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        let labels = ["fixture_db", "public", "buffered_table", "pg_default"].map(|label| {
            StrId(
                interner
                    .intern(label.as_bytes())
                    .expect("intern buffered table label")
                    .get(),
            )
        });
        for &(ts, datid, relid, counters) in rows {
            let mut row = buffered_user_table(ts, datid, relid, counters);
            [row.datname, row.schemaname, row.relname] = [labels[0], labels[1], labels[2]];
            row.tablespace = Some(labels[3]);
            buffers.push(row).expect("buffered table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode buffered table dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode buffered table snapshots")
            .expect("nonempty buffered table snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append buffered table snapshots");
    }

    pub(crate) fn append_named_index_snapshots(&mut self, rows: &[NamedIndexSnapshot<'_>]) {
        let mut interner = Interner::new(DictLimits::default());
        let tablespace = StrId(
            interner
                .intern(b"pg_default")
                .expect("intern tablespace")
                .get(),
        );
        let amname = StrId(interner.intern(b"btree").expect("intern AM").get());
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, indexrelid, idx_scan, datname, schema, table, index, definition) in rows {
            let mut row = user_index_v2(ts, datid, indexrelid, idx_scan);
            row.datname = StrId(
                interner
                    .intern(datname.as_bytes())
                    .expect("intern database")
                    .get(),
            );
            row.schemaname = StrId(
                interner
                    .intern(schema.as_bytes())
                    .expect("intern schema")
                    .get(),
            );
            row.relname = StrId(
                interner
                    .intern(table.as_bytes())
                    .expect("intern table")
                    .get(),
            );
            row.indexrelname = StrId(
                interner
                    .intern(index.as_bytes())
                    .expect("intern index")
                    .get(),
            );
            row.tablespace = Some(tablespace);
            row.amname = amname;
            row.indexdef = Some(StrId(
                interner
                    .intern(definition.as_bytes())
                    .expect("intern index definition")
                    .get(),
            ));
            buffers.push(row).expect("named index row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode index dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode named index snapshots")
            .expect("nonempty named index snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append named index snapshots");
    }

    fn append_placed_index_snapshots(&mut self, rows: &[PlacedIndexSnapshot<'_>]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(
            ts,
            datid,
            indexrelid,
            idx_scan,
            datname,
            schema,
            table,
            index,
            tablespace_oid,
            tablespace,
            main_fork_bytes,
        ) in rows
        {
            let mut row = user_index_v2(ts, datid, indexrelid, idx_scan);
            row.datname = fixture_label(&mut interner, datname);
            row.schemaname = fixture_label(&mut interner, schema);
            row.relname = fixture_label(&mut interner, table);
            row.indexrelname = fixture_label(&mut interner, index);
            row.tablespace_oid = tablespace_oid;
            row.tablespace = tablespace.map(|label| fixture_label(&mut interner, label));
            row.main_fork_bytes = main_fork_bytes;
            row.amname = fixture_label(&mut interner, "btree");
            buffers.push(row).expect("placed index row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("placed index dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode placed indexes")
            .expect("nonempty placed indexes");
        self.journal
            .append(self.address.id, &part)
            .expect("append placed indexes");
    }

    pub(crate) fn append_log_error(&mut self, at: i64) {
        self.append_log_error_count(at, 1);
    }

    pub(crate) fn append_log_error_count(&mut self, at: i64, count: u32) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(interner.intern(b"fixture").expect("intern label").get());
        let dictionary = dict::encode(interner.window()).expect("log dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgLogErrors {
                ts: Ts(at),
                system_identifier: None,
                source_file: label,
                severity: 0,
                category: 8,
                sqlstate: None,
                pattern: label,
                count,
                sample: label,
                detail: None,
                hint: None,
                context: None,
                statement: None,
                database: None,
                username: None,
            })
            .expect("log row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode log row")
            .expect("nonempty log row");
        self.journal
            .append(self.address.id, &part)
            .expect("append log row");
    }

    pub(crate) fn append_pgbouncer_event(&mut self, at: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(interner.intern(b"fixture").expect("intern label").get());
        let dictionary = dict::encode(interner.window()).expect("pgbouncer dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgBouncerEvents {
                ts: Ts(at),
                source_file: label,
                level: 2,
                database: None,
                username: None,
                host: None,
                text: label,
            })
            .expect("pgbouncer row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode pgbouncer row")
            .expect("nonempty pgbouncer row");
        self.journal
            .append(self.address.id, &part)
            .expect("append pgbouncer row");
    }

    /// Appends one 22-row mount snapshot whose active and finalized ordinals
    /// deliberately differ for `/pgdata` and `/victim`.
    pub(crate) fn append_reordering_mount_snapshot(&mut self, at: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for minor in 0..20 {
            let mount = if minor == 18 {
                "/victim".to_owned()
            } else {
                format!("/before-{minor:02}")
            };
            let mount_point = fixture_label(&mut interner, &mount);
            buffers
                .push(OsMountinfo {
                    ts: Ts(at),
                    major: 0,
                    minor,
                    mount_point,
                    root: mount_point,
                    fstype: fixture_label(&mut interner, "xfs"),
                    source: fixture_label(&mut interner, "/dev/fixture"),
                    is_k8s_infra: false,
                    total_bytes: Some(if minor == 18 { 900 } else { i64::from(minor) }),
                    free_bytes: None,
                    total_inodes: None,
                    available_inodes: None,
                    scope: 0,
                })
                .expect("mount row fits");
        }
        for (major, minor, mount, total_bytes) in [(8, 32, "/pgdata", 1_000), (9, 0, "/after", 1)] {
            let mount_point = if mount == "/pgdata" {
                StrId(
                    interner
                        .intern_blob(mount.as_bytes())
                        .expect("intern blob-backed mount")
                        .get(),
                )
            } else {
                fixture_label(&mut interner, mount)
            };
            buffers
                .push(OsMountinfo {
                    ts: Ts(at),
                    major,
                    minor,
                    mount_point,
                    root: mount_point,
                    fstype: fixture_label(&mut interner, "xfs"),
                    source: fixture_label(&mut interner, "/dev/fixture"),
                    is_k8s_infra: false,
                    total_bytes: Some(total_bytes),
                    free_bytes: None,
                    total_inodes: None,
                    available_inodes: None,
                    scope: 0,
                })
                .expect("mount row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("mount dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode mount rows")
            .expect("nonempty mount rows");
        self.journal
            .append(self.address.id, &part)
            .expect("append mount rows");
    }

    fn append_log_temp_file(&mut self, at: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let source_file = StrId(
            interner
                .intern(b"postgresql.log")
                .expect("intern source file")
                .get(),
        );
        let path = StrId(
            interner
                .intern(b"base/pgsql_tmp/pgsql_tmp42.0")
                .expect("intern temporary-file path")
                .get(),
        );
        let statement = StrId(
            interner
                .intern(b"select fixture spill")
                .expect("intern statement")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("temporary-file dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgLogTempFiles {
                ts: Ts(at),
                system_identifier: Some(42),
                source_file,
                path: Some(path),
                size_bytes: 1_048_576,
                statement: Some(statement),
            })
            .expect("temporary-file row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode temporary-file row")
            .expect("nonempty temporary-file row");
        self.journal
            .append(self.address.id, &part)
            .expect("append temporary-file row");
    }

    pub(crate) fn finish_and_continue(&mut self, segment_id: i64) {
        write_segment(&self.journal, &self.writer, self.address).expect("finish segment");
        self.journal.reset().expect("reset journal");
        self.address = SegmentAddress::new(SegmentId::new(segment_id).expect("segment id"))
            .expect("segment address");
    }

    fn append(&mut self, mut buffers: SectionBuffers) {
        let part = buffers
            .flush(&[])
            .expect("encode fixture part")
            .expect("nonempty fixture part");
        self.journal
            .append(self.address.id, &part)
            .expect("append fixture part");
    }

    pub(crate) fn finish(&self) {
        write_segment(&self.journal, &self.writer, self.address).expect("finish segment");
    }

    fn add_foreign_entry(&self) {
        std::fs::write(self.root().join("foreign"), b"fixture").expect("write foreign entry");
    }

    pub(crate) fn prepare(&self, target: &str, if_none_match: Option<&str>) -> Prepared {
        self.prepare_with_sources(target, if_none_match, SOURCES)
    }

    fn prepare_with_sources(
        &self,
        target: &str,
        if_none_match: Option<&str>,
        sources: u32,
    ) -> Prepared {
        let (path, query) = target
            .split_once('?')
            .map_or((target, None), |(path, query)| (path, Some(query)));
        let route = crate::route::parse(path, query).expect("valid fixture route");
        crate::api::prepare(self.root(), sources, route, if_none_match)
            .expect("prepare fixture resource")
    }
}

fn diskstats(ts: i64, minor: i32, reads: i64) -> OsDiskstats {
    diskstats_with_device(ts, minor, reads, StrId(999))
}

fn fixture_label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern fixture label")
            .get(),
    )
}

fn user_table(ts: i64, datid: u32, relid: u32, seq_scan: i64) -> PgStatUserTablesV1 {
    PgStatUserTablesV1 {
        ts: Ts(ts),
        datid,
        datname: StrId(901),
        relid,
        schemaname: StrId(902),
        relname: StrId(903),
        tablespace_oid: Some(1_663),
        tablespace: Some(StrId(904)),
        seq_scan,
        seq_tup_read: 0,
        idx_scan: None,
        idx_tup_fetch: None,
        n_tup_ins: 0,
        n_tup_upd: 0,
        n_tup_del: 0,
        n_tup_hot_upd: 0,
        n_live_tup: 0,
        n_dead_tup: 0,
        n_mod_since_analyze: 0,
        vacuum_count: 0,
        autovacuum_count: 0,
        analyze_count: 0,
        autoanalyze_count: 0,
        last_vacuum: None,
        last_autovacuum: None,
        last_analyze: None,
        last_autoanalyze: None,
        main_fork_bytes: 0,
        toast_bytes: None,
        toast_n_live_tup: None,
        toast_n_dead_tup: None,
        toast_last_autovacuum: None,
        xid_age: None,
        mxid_age: None,
        reltuples: 0,
        heap_blks_read: 0,
        heap_blks_hit: 0,
        idx_blks_read: None,
        idx_blks_hit: None,
        toast_blks_read: None,
        toast_blks_hit: None,
        tidx_blks_read: None,
        tidx_blks_hit: None,
    }
}

fn postgres_database(
    ts: i64,
    xact_commit: i64,
    xact_rollback: i64,
    deadlocks: i64,
) -> PgStatDatabaseV1 {
    PgStatDatabaseV1 {
        ts: Ts(ts),
        datid: 73,
        datname: Some(StrId(901)),
        numbackends: Some(5),
        xact_commit,
        xact_rollback,
        blks_read: 0,
        blks_hit: 0,
        tup_returned: 0,
        tup_fetched: 0,
        tup_inserted: 0,
        tup_updated: 0,
        tup_deleted: 0,
        conflicts: 0,
        temp_files: 0,
        temp_bytes: 0,
        deadlocks,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        stats_reset: None,
        frozen_xid_age: Some(1),
        min_mxid_age: Some(1),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
    }
}

fn buffered_user_table(ts: i64, datid: u32, relid: u32, buffers: [i64; 8]) -> PgStatUserTablesV1 {
    let mut row = user_table(ts, datid, relid, 0);
    let [
        heap_read,
        heap_hit,
        index_read,
        index_hit,
        toast_read,
        toast_hit,
        toast_index_read,
        toast_index_hit,
    ] = buffers;
    row.heap_blks_read = heap_read;
    row.heap_blks_hit = heap_hit;
    row.idx_blks_read = Some(index_read);
    row.idx_blks_hit = Some(index_hit);
    row.toast_blks_read = Some(toast_read);
    row.toast_blks_hit = Some(toast_hit);
    row.tidx_blks_read = Some(toast_index_read);
    row.tidx_blks_hit = Some(toast_index_hit);
    row
}

fn user_index_v1(ts: i64, datid: u32, indexrelid: u32, idx_scan: i64) -> PgStatUserIndexesV1 {
    PgStatUserIndexesV1 {
        ts: Ts(ts),
        datid,
        datname: StrId(901),
        indexrelid,
        relid: indexrelid - 1,
        schemaname: StrId(902),
        relname: StrId(903),
        indexrelname: StrId(905),
        tablespace_oid: 1_663,
        tablespace: Some(StrId(904)),
        idx_scan,
        idx_tup_read: 0,
        idx_tup_fetch: 0,
        main_fork_bytes: 0,
        indisunique: false,
        indisprimary: false,
        indisvalid: true,
        indisexclusion: false,
        indisready: true,
        amname: StrId(906),
        indexdef: None,
        idx_blks_read: 0,
        idx_blks_hit: 0,
    }
}

fn user_index_v2(ts: i64, datid: u32, indexrelid: u32, idx_scan: i64) -> PgStatUserIndexesV2 {
    let base = user_index_v1(ts, datid, indexrelid, idx_scan);
    PgStatUserIndexesV2 {
        ts: base.ts,
        datid: base.datid,
        datname: base.datname,
        indexrelid: base.indexrelid,
        relid: base.relid,
        schemaname: base.schemaname,
        relname: base.relname,
        indexrelname: base.indexrelname,
        tablespace_oid: base.tablespace_oid,
        tablespace: base.tablespace,
        idx_scan: base.idx_scan,
        idx_tup_read: base.idx_tup_read,
        idx_tup_fetch: base.idx_tup_fetch,
        main_fork_bytes: base.main_fork_bytes,
        last_idx_scan: None,
        indisunique: base.indisunique,
        indisprimary: base.indisprimary,
        indisvalid: base.indisvalid,
        indisexclusion: base.indisexclusion,
        indisready: base.indisready,
        amname: base.amname,
        indexdef: base.indexdef,
        idx_blks_read: base.idx_blks_read,
        idx_blks_hit: base.idx_blks_hit,
    }
}

fn diskstats_with_device(ts: i64, minor: i32, reads: i64, device: StrId) -> OsDiskstats {
    OsDiskstats {
        ts: Ts(ts),
        major: 8,
        minor,
        device,
        reads,
        r_merged: 0,
        read_sectors: reads,
        read_time_ms: 0,
        writes: 0,
        w_merged: 0,
        write_sectors: 0,
        write_time_ms: 0,
        io_in_progress: 0,
        io_time_ms: 0,
        io_weighted_time_ms: 0,
        discards: None,
        d_merged: None,
        discard_sectors: None,
        discard_time_ms: None,
        flushes: None,
        flush_time_ms: None,
        scope: 0,
    }
}

fn netdev(ts: i64, iface: StrId, rx_bytes: i64) -> OsNetdev {
    OsNetdev {
        ts: Ts(ts),
        iface,
        rx_bytes,
        rx_packets: 0,
        rx_errs: 0,
        rx_drop: 0,
        rx_fifo: 0,
        rx_frame: 0,
        rx_compressed: 0,
        rx_multicast: 0,
        tx_bytes: 0,
        tx_packets: 0,
        tx_errs: 0,
        tx_drop: 0,
        tx_fifo: 0,
        tx_colls: 0,
        tx_carrier: 0,
        tx_compressed: 0,
        speed_mbit: None,
        duplex: 0,
        scope: 0,
    }
}

fn process(ts: i64, read_bytes: Option<i64>, label: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(ts),
        pid: 41,
        starttime: Ts(SEGMENT_ID - 1_000_000),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'S',
        num_threads: 1,
        tty: 0,
        comm: label,
        cmdline: Some(label),
        utime: 0,
        stime: 0,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 0,
        rundelay_ns: 0,
        blkdelay_ticks: 0,
        nvcsw: 0,
        nivcsw: 0,
        minflt: 0,
        majflt: 0,
        vmem_kb: 0,
        rmem_kb: 0,
        vswap_kb: 0,
        syscr: None,
        syscw: None,
        rchar: None,
        wchar: None,
        read_bytes,
        write_bytes: None,
        cancelled_write_bytes: None,
        exit_signal: 17,
        scope: 0,
    }
}

fn process_for_summary(ts: i64, pid: i32, counter: i64, label: StrId) -> OsProcess {
    let mut row = process(ts, Some(counter + i64::from(pid)), label);
    row.pid = pid;
    row.starttime = Ts(SEGMENT_ID - 1_000_000 + i64::from(pid));
    row.state = if pid % 2 == 0 { b'R' } else { b'S' };
    row.num_threads = 2;
    row.utime = counter + i64::from(pid);
    row.stime = counter * 2 + i64::from(pid);
    row.rundelay_ns = counter * 1_000_000 + i64::from(pid);
    row.nvcsw = counter + i64::from(pid);
    row.nivcsw = counter * 2 + i64::from(pid);
    row.rmem_kb = 10 + i64::from(pid);
    row.vmem_kb = 20 + i64::from(pid);
    row.vswap_kb = i64::from(pid % 3);
    row.majflt = counter + i64::from(pid);
    row.syscr = Some(counter + i64::from(pid));
    row.syscw = Some(counter * 2 + i64::from(pid));
    row.write_bytes = None;
    row
}

fn statement(ts: i64, calls: i64, total_exec_time: f64, label: StrId) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(ts),
        queryid: Some(71),
        userid: 72,
        dbid: 73,
        datname: None,
        usename: None,
        query: Some(label),
        calls,
        rows: 0,
        plans: 0,
        total_exec_time,
        total_plan_time: 0.0,
        min_exec_time: 0.0,
        max_exec_time: 0.0,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
        stddev_plan_time: 0.0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        wal_records: 0,
        wal_fpi: 0,
        wal_bytes: 0,
    }
}

fn store_plan(ts: i64, queryid: i64, plan: StrId) -> PgStorePlansOsscV1 {
    PgStorePlansOsscV1 {
        ts: Ts(ts),
        queryid,
        planid: -7,
        userid: 10,
        dbid: 11,
        datname: None,
        usename: None,
        plan: Some(plan),
        calls: 4,
        total_time: 99.5,
        min_time: 1.0,
        max_time: 50.0,
        mean_time: 24.9,
        stddev_time: 2.2,
        rows: 40,
        shared_blks_hit: 1,
        shared_blks_read: 2,
        shared_blks_dirtied: 3,
        shared_blks_written: 4,
        local_blks_hit: 5,
        local_blks_read: 6,
        local_blks_dirtied: 7,
        local_blks_written: 8,
        temp_blks_read: 9,
        temp_blks_written: 10,
        shared_blk_read_time: 1.5,
        shared_blk_write_time: 2.5,
        local_blk_read_time: 3.5,
        local_blk_write_time: 4.5,
        temp_blk_read_time: 5.5,
        temp_blk_write_time: 6.5,
        first_call: Ts(ts - 1_000),
        last_call: Ts(ts - 1),
    }
}

fn store_plan_vadv(ts: i64, plan: StrId) -> PgStorePlansVadvV1 {
    PgStorePlansVadvV1 {
        ts: Ts(ts),
        userid: 10,
        dbid: 11,
        queryid: 1,
        planid: -7,
        queryid_stat_statements: 1,
        datname: None,
        usename: None,
        plan: Some(plan),
        calls: 0,
        slow_log_calls: 0,
        total_time: 0.0,
        min_time: 1.0,
        max_time: 2.0,
        mean_time: 1.5,
        stddev_time: 0.5,
        rows: 0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        first_call: Ts(50),
        last_call: Ts(ts),
        total_plan_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
    }
}

#[test]
fn real_rows_keep_js_unsafe_integers_and_truncated_blob_metadata_lossless() {
    let mut fixture = Fixture::new();
    fixture.append_blob_diskstats(&[0xff; 17], i64::MAX);

    let records = stream(fixture.prepare(&target("history", "field=device&field=reads"), None))
        .expect("blob history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][1], i64::MAX.to_string());
    let blob = &rows[0]["values"][0];
    assert_eq!(blob["representation"], "bytes");
    assert_eq!(blob["full_len"], "17");
    assert_eq!(blob["truncated"], true);
    assert_eq!(
        blob["stored_base64"].as_str().map(str::len),
        Some(24),
        "all 16 stored bytes survive in base64"
    );
    assert_eq!(
        blob["sha256"].as_str().map(str::len),
        Some(64),
        "the full-value SHA-256 survives"
    );
}

#[test]
fn cancellation_stops_a_history_scan_before_dictionary_work() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    let prepared = fixture.prepare(&target("history", "field=reads"), None);
    let checks = Cell::new(0_u8);
    let mut records = Vec::new();

    prepared
        .stream(
            &mut |record| {
                records.push(record);
                true
            },
            &|| {
                let next = checks.get().saturating_add(1);
                checks.set(next);
                next >= 3
            },
        )
        .expect("cancelled scan exits normally");

    assert_eq!(records.len(), 2, "only the header and layout are emitted");
    assert!(
        records
            .iter()
            .all(|record| !record.starts_with(b"{\"record\":\"row\""))
    );
}

fn psi(ts: i64, resource: u8, some_total: i64) -> OsPsi {
    OsPsi {
        ts: Ts(ts),
        resource,
        some_avg10: 0.0,
        some_avg60: 0.0,
        some_avg300: 0.0,
        some_total,
        full_avg10: None,
        full_avg60: None,
        full_avg300: None,
        full_total: None,
        scope: 0,
    }
}

fn activity(ts: i64, pid: i32, state: StrId, query: StrId) -> PgStatActivityV3 {
    PgStatActivityV3 {
        ts: Ts(ts),
        pid,
        leader_pid: None,
        datid: None,
        datname: None,
        usename: None,
        application_name: state,
        client_addr: state,
        backend_type: state,
        state: Some(state),
        wait_event_type: Some(state),
        wait_event: Some(state),
        query: Some(query),
        query_id: None,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Ts(1),
        xact_start: None,
        query_start: None,
        state_change: None,
    }
}

fn stream(prepared: Prepared) -> Result<Vec<Value>, ApiError> {
    let mut records = Vec::new();
    prepared.stream(
        &mut |record| {
            records.push(record);
            true
        },
        &|| false,
    )?;
    Ok(records
        .iter()
        .map(|record| serde_json::from_slice(record).expect("JSON record"))
        .collect())
}

fn event_detail(fixture: &Fixture, item: &Value) -> Value {
    let detail_ref = item["detail_ref"].as_str().expect("opaque detail ref");
    let target = format!("/api/row-detail?detail_ref={detail_ref}");
    let mut records = stream(fixture.prepare(&target, None)).expect("stream row detail");
    assert_eq!(records.len(), 1);
    records.remove(0)
}

fn prepare_result(fixture: &Fixture, target: &str) -> Result<Prepared, ApiError> {
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(path, query)| (path, Some(query)));
    let route = match crate::route::parse(path, query) {
        Ok(route) => route,
        Err(error) => panic!("valid fixture route: {error}"),
    };
    crate::api::prepare(fixture.root(), SOURCES, route, None)
}

fn source_flags(records: &[Value], name: &str) -> (bool, bool) {
    let family = records
        .iter()
        .find(|record| record["record"] == "catalog")
        .and_then(|record| record["source_families"].as_array())
        .and_then(|families| families.iter().find(|family| family["name"] == name))
        .expect("source family");
    (
        family["present"].as_bool().expect("present flag"),
        family["metrics_present"]
            .as_bool()
            .expect("metrics-present flag"),
    )
}

fn row_records(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect()
}

fn relation_records(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["record"] == "relation")
        .collect()
}

fn target(resource: &str, query: &str) -> String {
    format!("/api/segments/{SEGMENT_ID}/sections/os_diskstats/{resource}?{query}")
}

fn accepted(value: &str) -> AcceptedEncodings {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_str(value).expect("valid Accept-Encoding"),
    );
    AcceptedEncodings::from_headers(&headers).expect("acceptable representation")
}

#[tokio::test]
async fn large_ndjson_gzip_round_trips_to_the_identity_representation() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..256)
            .map(|minor| (100, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    let resource = target("history", "field=reads");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), AcceptedEncodings::default()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    assert_eq!(
        response.headers().get(VARY),
        Some(&HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
    );
    let mut compressed = Vec::new();
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let bytes = frame
            .expect("gzip body frame")
            .into_data()
            .expect("data frame");
        assert!(bytes.len() <= 8 * 1_024, "bounded gzip frame");
        compressed.extend_from_slice(&bytes);
    }
    assert_eq!(compressed.get(4..8), Some([0, 0, 0, 0].as_slice()));
    let mut decoded = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut decoded)
        .expect("decode response gzip");

    let prepared = fixture.prepare(&resource, None);
    let identity = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    assert_eq!(identity.status(), StatusCode::OK);
    assert!(!identity.headers().contains_key(CONTENT_ENCODING));
    let identity = identity
        .into_body()
        .collect()
        .await
        .expect("identity body")
        .to_bytes();
    assert!(identity.len() > 8 * 1_024, "fixture crosses the threshold");
    assert_eq!(decoded, identity);
}

#[tokio::test]
async fn below_threshold_ndjson_stays_identity_when_it_is_allowed() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    let prepared = fixture.prepare(&target("history", "field=reads"), None);
    let response = crate::blocking_stream(move || Ok(prepared), AcceptedEncodings::default()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("small identity body")
        .to_bytes();
    assert!(body.len() < 8 * 1_024);
    let first = body
        .split(|byte| *byte == b'\n')
        .next()
        .expect("history header");
    assert_eq!(
        serde_json::from_slice::<Value>(first).expect("history JSON")["record"],
        "history"
    );

    let prepared = fixture.prepare(&target("history", "field=reads"), None);
    let response =
        crate::blocking_stream(move || Ok(prepared), accepted("gzip, identity;q=0")).await;
    assert_eq!(
        response.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    let compressed = response
        .into_body()
        .collect()
        .await
        .expect("forced gzip body")
        .to_bytes();
    let mut decoded = Vec::new();
    GzDecoder::new(compressed.as_ref())
        .read_to_end(&mut decoded)
        .expect("decode forced gzip");
    assert_eq!(decoded, body);
}

#[tokio::test]
async fn an_active_snapshot_restarts_from_the_finished_segment_after_rollover() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=instance_metadata&field=postgresql_interval_seconds&text=160"
    );
    let (path, query) = target.split_once('?').expect("snapshot query");
    let route = crate::route::parse(path, Some(query)).expect("snapshot route");
    let first = crate::api::prepare(fixture.root(), SOURCES, route.clone(), None)
        .expect("prepare active snapshot");
    fixture.finish_and_continue(SEGMENT_ID + 1_000);

    let root = fixture.root().to_owned();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&attempts);
    let mut first = Some(first);
    let response = crate::blocking_stream_with_replay(
        move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(first) = first.take() {
                return Ok(first);
            }
            crate::api::prepare(&root, SOURCES, route.clone(), None)
        },
        accepted("identity"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(
        response.headers().get(hyper::header::CACHE_CONTROL),
        Some(&HeaderValue::from_static(
            "private,max-age=31536000,immutable"
        )),
        "the finished retry controls the response cache policy"
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("replayed snapshot body")
        .to_bytes();
    let records = body
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).expect("snapshot JSON"))
        .collect::<Vec<_>>();
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(rows[0]["values"], serde_json::json!(["30"]));
}

#[tokio::test]
async fn a_started_active_response_is_not_spliced_to_a_new_generation() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..2_000)
            .map(|minor| (100, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    fixture.append_diskstats(&[(100, 2_000, 2_000)]);
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&text=160"
    );
    let (path, query) = target.split_once('?').expect("snapshot query");
    let route = crate::route::parse(path, Some(query)).expect("snapshot route");
    let first = crate::api::prepare(fixture.root(), SOURCES, route.clone(), None)
        .expect("prepare large active snapshot");
    let root = fixture.root().to_owned();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&attempts);
    let mut first = Some(first);
    let response = crate::blocking_stream_with_replay(
        move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(first) = first.take() {
                return Ok(first);
            }
            crate::api::prepare(&root, SOURCES, route.clone(), None)
        },
        accepted("identity"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 1);

    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    assert!(
        response.into_body().collect().await.is_err(),
        "an already-started response must end instead of mixing segment generations"
    );
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn a_small_real_read_failure_returns_500_before_success_headers() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    let prepared = fixture.prepare(&target("history", "field=device"), None);
    let response = crate::blocking_stream(move || Ok(prepared), AcceptedEncodings::default()).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.headers().get(CONTENT_TYPE),
        Some(&HeaderValue::from_static("application/json"))
    );
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("ordinary error body")
        .to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("error JSON"),
        serde_json::json!({"error": "unreadable"})
    );
}

#[tokio::test]
async fn a_real_read_failure_after_a_valid_prefix_fails_the_body_without_a_trailer() {
    let mut fixture = Fixture::new();
    fixture.append_named_then_unreadable_diskstats(160);
    let resource = target("history", "field=device");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let error = response
        .into_body()
        .collect()
        .await
        .expect_err("late read failure reaches the body");
    assert_eq!(error.to_string(), "response body failed");

    let prepared = fixture.prepare(&resource, None);
    let response =
        crate::blocking_stream(move || Ok(prepared), accepted("gzip, identity;q=0")).await;
    assert_eq!(
        response.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    response
        .into_body()
        .collect()
        .await
        .expect_err("late read failure also aborts gzip");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    let mut prefix = Vec::new();
    let mut failure = None;
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(frame) => prefix.extend_from_slice(&frame.into_data().expect("data frame")),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    assert!(failure.is_some(), "body ends with an error");
    assert!(prefix.len() >= 8 * 1_024, "a valid prefix was committed");
    assert!(
        !prefix
            .windows(b"\"record\":\"error\"".len())
            .any(|window| { window == b"\"record\":\"error\"" })
    );
    for record in prefix
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        serde_json::from_slice::<Value>(record).expect("prefix contains complete NDJSON records");
    }
}

#[test]
fn explicit_history_projection_keeps_coordinates_without_resolving_unused_text() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);

    let projected = fixture.prepare(&target("history", "field=reads"), None);
    assert_eq!(projected.meta().cache, CachePolicy::NoStore);
    let records = stream(projected).expect("projected history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["timestamp"], "100");
    assert_eq!(rows[0]["identity"], serde_json::json!([8, 0]));
    assert_eq!(rows[0]["values"], serde_json::json!(["7"]));

    let requested_text = fixture.prepare(&target("history", "field=device"), None);
    let error = requested_text
        .stream(&mut |_record| true, &|| false)
        .expect_err("requested corrupt dictionary id must fail");
    assert!(matches!(error, ApiError::Unreadable(_)));
    assert!(error.to_string().contains("unresolved dictionary id 999"));

    let requested_filter =
        fixture.prepare(&target("history", "field=reads&where.device=sda"), None);
    let error = requested_filter
        .stream(&mut |_record| true, &|| false)
        .expect_err("corrupt dictionary id in a filter must fail");
    assert!(matches!(error, ApiError::Unreadable(_)));
    assert!(error.to_string().contains("unresolved dictionary id 999"));
}

#[test]
fn active_pages_pin_valid_len_and_preserve_equal_timestamp_ordinals() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 10), (100, 1, 11)]);
    let first_position = fixture.position();

    let first = stream(fixture.prepare(&target("rows", "field=reads&page_size=1&order=asc"), None))
        .expect("first page");
    assert_eq!(
        first[0]["segment"]["cursor"]["wal_position"],
        first_position.to_string()
    );
    let first_rows = row_records(&first);
    assert_eq!(first_rows.len(), 1);
    assert_eq!(first_rows[0]["ordinal"], "0");
    let cursor = first
        .iter()
        .find(|record| record["record"] == "page")
        .and_then(|record| record["next_cursor"].as_str())
        .expect("page cursor")
        .to_owned();

    fixture.append_diskstats(&[(100, 2, 12)]);

    let resumed = stream(fixture.prepare(
        &target(
            "rows",
            &format!("field=reads&page_size=1&order=asc&cursor={cursor}"),
        ),
        None,
    ))
    .expect("resume pinned page");
    let resumed_rows = row_records(&resumed);
    assert_eq!(resumed_rows.len(), 1);
    assert_eq!(resumed_rows[0]["ordinal"], "1");
    assert_eq!(
        resumed
            .iter()
            .find(|record| record["record"] == "page")
            .expect("page trailer")["next_cursor"],
        Value::Null
    );

    let fresh =
        stream(fixture.prepare(&target("rows", "field=reads&page_size=100&order=asc"), None))
            .expect("fresh page");
    assert_eq!(
        row_records(&fresh)
            .iter()
            .map(|row| row["ordinal"].as_str().expect("ordinal"))
            .collect::<Vec<_>>(),
        ["0", "1", "2"]
    );

    let tail = stream(fixture.prepare(
        &target(
            "history",
            &format!("field=reads&after={SEGMENT_ID},{first_position}"),
        ),
        None,
    ))
    .expect("active history tail");
    let tail_rows = row_records(&tail);
    assert_eq!(tail_rows.len(), 1);
    assert_eq!(tail_rows[0]["ordinal"], "2");
    assert_eq!(tail_rows[0]["timestamp"], "100");
    assert_eq!(tail_rows[0]["identity"], serde_json::json!([8, 2]));
}

#[test]
fn finished_index_and_catalog_have_revalidation_contracts_and_source_facts() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 0, 9)]);
    fixture.append_health();
    fixture.finish();

    let index_target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    let prepared = fixture.prepare(&index_target, None);
    let meta = prepared.meta();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::Immutable);
    let etag = meta.etag.expect("finished index ETag");
    let index = stream(prepared).expect("finished index body");
    assert!(index.iter().any(|record| record["record"] == "point"));

    assert!(etag.starts_with("W/\""));
    let strong = etag.strip_prefix("W/").expect("weak validator");
    let offered = format!("\"stale\", {strong}");
    let not_modified = fixture.prepare(&index_target, Some(&offered));
    assert_eq!(not_modified.meta().status, StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.meta().cache, CachePolicy::Immutable);
    assert_eq!(not_modified.meta().etag.as_deref(), Some(etag.as_str()));
    assert!(matches!(not_modified, Prepared::Empty(_)));

    let catalog = fixture.prepare("/api/catalog", None);
    assert_eq!(catalog.meta().cache, CachePolicy::Revalidate);
    assert_eq!(catalog.meta().etag, None);
    let catalog = stream(catalog).expect("catalog body");
    let header = catalog
        .iter()
        .find(|record| record["record"] == "catalog")
        .expect("catalog header");
    let families = header["source_families"]
        .as_array()
        .expect("source families");
    let os = families
        .iter()
        .find(|family| family["name"] == "os")
        .expect("OS source family");
    assert_eq!(os["configured"], true);
    assert_eq!(os["present"], true);
    assert_eq!(os["metrics_present"], true);
    let postgresql = families
        .iter()
        .find(|family| family["name"] == "postgresql")
        .expect("PostgreSQL source family");
    assert_eq!(postgresql["configured"], true);
    assert_eq!(postgresql["present"], false);
    assert_eq!(postgresql["metrics_present"], false);

    let history = fixture.prepare(&target("history", "field=reads"), None);
    assert_eq!(history.meta().cache, CachePolicy::Immutable);
    assert_eq!(
        row_records(&stream(history).expect("finished history")).len(),
        2
    );
}

#[test]
fn catalog_includes_the_active_wal_cursor() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 0, 9)]);
    let position = fixture.position();

    let bytes = catalog_response_bytes(&fixture, "/api/catalog", usize::MAX, usize::MAX)
        .expect("stream active catalog");
    let records = raw_ndjson_records(&bytes);
    let active = records
        .iter()
        .find(|record| record["record"] == "active_segment")
        .expect("active catalog record");
    assert_eq!(active["id"], SEGMENT_ID.to_string());
    assert_eq!(active["cursor"]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(active["cursor"]["wal_position"], position.to_string());
}

#[test]
fn catalog_prefers_finished_over_active_for_the_same_generation() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    fixture.finish();

    let bytes = catalog_response_bytes(&fixture, "/api/catalog", usize::MAX, usize::MAX)
        .expect("stream finished catalog");
    let segments = raw_ndjson_records(&bytes)
        .into_iter()
        .filter(|record| {
            matches!(
                record["record"].as_str(),
                Some("finished_segment" | "active_segment")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0]["record"], "finished_segment");
    assert_eq!(segments[0]["id"], SEGMENT_ID.to_string());
}

#[test]
fn catalog_orders_segments_inside_the_requested_bounds() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 1);
    fixture.append_diskstats(&[(200, 0, 2)]);
    fixture.finish_and_continue(SEGMENT_ID + 2);
    fixture.append_diskstats(&[(300, 0, 3)]);
    fixture.finish_and_continue(SEGMENT_ID + 3);
    fixture.append_diskstats(&[(400, 0, 4)]);

    let bytes = catalog_response_bytes(
        &fixture,
        "/api/catalog?from=150&to=350",
        usize::MAX,
        usize::MAX,
    )
    .expect("stream bounded catalog");

    let segment_ids = raw_ndjson_records(&bytes)
        .into_iter()
        .filter(|record| record["record"] == "finished_segment")
        .map(|record| record["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        segment_ids,
        [SEGMENT_ID + 1, SEGMENT_ID + 2].map(|id| Value::from(id.to_string()))
    );
}

#[test]
fn catalog_emits_warnings_after_segments() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish();
    fixture.add_foreign_entry();

    let bytes = catalog_response_bytes(&fixture, "/api/catalog", usize::MAX, usize::MAX)
        .expect("stream warning catalog");
    let records = raw_ndjson_records(&bytes);
    let warning = records.last().expect("warning record");
    assert_eq!(warning["record"], "warning");
    assert_eq!(warning["affected"]["kind"], "foreign_entry");
}

#[test]
fn catalog_honors_cancellation_and_sink_stop_boundaries() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 1);
    fixture.append_diskstats(&[(200, 0, 2)]);
    fixture.finish();
    fixture.add_foreign_entry();

    for (accepted_records, cancelled_after, expected) in [
        (usize::MAX, 0, &[][..]),
        (usize::MAX, 1, &["catalog"][..]),
        (1, usize::MAX, &["catalog"][..]),
        (2, usize::MAX, &["catalog", "finished_segment"][..]),
    ] {
        let bytes =
            catalog_response_bytes(&fixture, "/api/catalog", accepted_records, cancelled_after)
                .expect("stop catalog stream");
        assert_eq!(
            raw_ndjson_records(&bytes)
                .iter()
                .map(|record| record["record"].as_str().expect("record kind"))
                .collect::<Vec<_>>(),
            expected,
            "accepted_records={accepted_records} cancelled_after={cancelled_after}"
        );
    }
}

fn catalog_response_bytes(
    fixture: &Fixture,
    target: &str,
    accepted_records: usize,
    cancelled_after: usize,
) -> Result<Vec<u8>, ApiError> {
    let emitted = Cell::new(0_usize);
    let mut output = Vec::new();
    fixture.prepare(target, None).stream(
        &mut |bytes| {
            output.extend_from_slice(&bytes);
            emitted.set(emitted.get() + 1);
            emitted.get() < accepted_records
        },
        &|| emitted.get() >= cancelled_after,
    )?;
    Ok(output)
}

fn raw_ndjson_records(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| serde_json::from_slice(record).expect("JSON record"))
        .collect()
}

#[test]
fn history_orders_finished_rows_by_physical_ordinal() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 1, 9), (300, 2, 11)]);
    fixture.finish();
    let target = target("history", "field=minor&field=reads");

    let bytes = query_response_bytes(&fixture, &target, usize::MAX, usize::MAX)
        .expect("stream finished history");

    assert_eq!(
        raw_ndjson_records(&bytes)
            .into_iter()
            .filter(|record| record["record"] == "row")
            .map(|record| record["ordinal"].clone())
            .collect::<Vec<_>>(),
        ["0", "1", "2"].map(Value::from)
    );
}

#[test]
fn history_starts_after_the_active_cursor() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 1, 9)]);
    let position = fixture.position();
    fixture.append_diskstats(&[(300, 2, 11)]);
    let target = target(
        "history",
        &format!("field=reads&after={SEGMENT_ID},{position}"),
    );

    let bytes = query_response_bytes(&fixture, &target, usize::MAX, usize::MAX)
        .expect("stream active tail");

    let records = raw_ndjson_records(&bytes);
    let header = records
        .iter()
        .find(|record| record["record"] == "history")
        .expect("history header");
    assert_eq!(header["after"]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(header["after"]["wal_position"], position.to_string());
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ordinal"], "2");
    assert_eq!(rows[0]["values"], serde_json::json!(["11"]));
}

#[test]
fn rows_page_finished_data_in_both_orders() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..5)
            .map(|minor| (100, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    fixture.finish();

    for (order, expected) in [
        ("asc", vec!["0", "1", "2", "3", "4"]),
        ("desc", vec!["4", "3", "2", "1", "0"]),
    ] {
        let base = target(
            "rows",
            &format!("field=minor&field=reads&page_size=2&order={order}"),
        );
        let mut cursor = None;
        let mut ordinals = Vec::new();
        let mut page_count = 0_usize;
        loop {
            let target = cursor
                .as_ref()
                .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
            let bytes = query_response_bytes(&fixture, &target, usize::MAX, usize::MAX)
                .expect("stream finished rows page");

            let records = raw_ndjson_records(&bytes);
            ordinals.extend(
                records
                    .iter()
                    .filter(|record| record["record"] == "row")
                    .map(|record| record["ordinal"].as_str().expect("row ordinal").to_owned()),
            );
            cursor = records
                .iter()
                .find(|record| record["record"] == "page")
                .expect("page trailer")["next_cursor"]
                .as_str()
                .map(ToOwned::to_owned);
            page_count += 1;
            if cursor.is_none() {
                break;
            }
            assert!(page_count < 4, "cursor must make progress");
        }
        assert_eq!(ordinals, expected, "order={order}");
        assert_eq!(page_count, 3, "order={order}");
    }
}

#[test]
fn rows_cursor_pins_the_active_position() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 10), (100, 1, 11), (100, 2, 12)]);
    let position = fixture.position();
    let base = target("rows", "field=reads&page_size=2&order=asc");

    let bytes = query_response_bytes(&fixture, &base, usize::MAX, usize::MAX)
        .expect("stream first active page");
    let first = raw_ndjson_records(&bytes);
    let cursor = first
        .iter()
        .find(|record| record["record"] == "page")
        .and_then(|record| record["next_cursor"].as_str())
        .expect("active page cursor")
        .to_owned();
    assert_eq!(
        first
            .iter()
            .find(|record| record["record"] == "rows")
            .expect("rows header")["segment"]["cursor"]["wal_position"],
        position.to_string()
    );

    fixture.append_diskstats(&[(100, 3, 13)]);
    let resumed_target = format!("{base}&cursor={cursor}");
    let bytes = query_response_bytes(&fixture, &resumed_target, usize::MAX, usize::MAX)
        .expect("stream pinned active page");
    let resumed = raw_ndjson_records(&bytes);
    let rows = resumed
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ordinal"], "2");
    assert_eq!(
        resumed
            .iter()
            .find(|record| record["record"] == "page")
            .expect("page trailer")["next_cursor"],
        Value::Null
    );
}

#[test]
fn history_and_rows_return_stable_errors() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    fixture.finish();

    for (target, status, code, parameter, message) in [
        (
            target("history", "field=not_recorded"),
            StatusCode::BAD_REQUEST,
            "no_such_column",
            Some("not_recorded"),
            "no such column \"not_recorded\"",
        ),
        (
            format!("/api/segments/{SEGMENT_ID}/sections/not_recorded/history?field=reads"),
            StatusCode::NOT_FOUND,
            "no_such_section",
            None,
            "no such logical section",
        ),
    ] {
        let error = query_response_bytes(&fixture, &target, usize::MAX, usize::MAX)
            .expect_err("history must reject request");
        assert_api_error(&error, status, code, parameter, message);
    }

    for (target, status, code, parameter, message) in [
        (
            target("rows", "field=not_recorded"),
            StatusCode::BAD_REQUEST,
            "no_such_column",
            Some("not_recorded"),
            "no such column \"not_recorded\"",
        ),
        (
            target("rows", "field=reads&cursor=not-a-cursor"),
            StatusCode::BAD_REQUEST,
            "bad_cursor",
            None,
            "invalid page cursor",
        ),
    ] {
        let error = query_response_bytes(&fixture, &target, usize::MAX, usize::MAX)
            .expect_err("rows must reject request");
        assert_api_error(&error, status, code, parameter, message);
    }
}

#[test]
fn history_and_rows_honor_cancellation_and_sink_stop_boundaries() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 1, 9), (300, 2, 11)]);
    fixture.finish();
    let history = target("history", "field=minor&field=reads");
    let rows = target("rows", "field=minor&field=reads&page_size=2&order=desc");

    for (accepted_records, cancelled_after, expected) in [
        (usize::MAX, 0, &[][..]),
        (usize::MAX, 1, &["history"][..]),
        (1, usize::MAX, &["history"][..]),
        (2, usize::MAX, &["history", "layout"][..]),
        (3, usize::MAX, &["history", "layout", "row"][..]),
    ] {
        let bytes = query_response_bytes(&fixture, &history, accepted_records, cancelled_after)
            .expect("stop history stream");
        assert_eq!(
            record_kinds(&bytes),
            expected,
            "history accepted_records={accepted_records} cancelled_after={cancelled_after}"
        );

        let expected = expected
            .iter()
            .map(|kind| if *kind == "history" { "rows" } else { *kind })
            .collect::<Vec<_>>();
        let bytes = query_response_bytes(&fixture, &rows, accepted_records, cancelled_after)
            .expect("stop rows stream");
        assert_eq!(
            record_kinds(&bytes),
            expected,
            "rows accepted_records={accepted_records} cancelled_after={cancelled_after}"
        );
    }
}

fn query_response_bytes(
    fixture: &Fixture,
    target: &str,
    accepted_records: usize,
    cancelled_after: usize,
) -> Result<Vec<u8>, ApiError> {
    query_response(fixture, target, accepted_records, cancelled_after).map(|(_meta, bytes)| bytes)
}

fn query_response(
    fixture: &Fixture,
    target: &str,
    accepted_records: usize,
    cancelled_after: usize,
) -> Result<(ResponseMeta, Vec<u8>), ApiError> {
    let emitted = Cell::new(0_usize);
    let mut output = Vec::new();
    let prepared = prepare_result(fixture, target)?;
    let meta = prepared.meta();
    prepared.stream(
        &mut |bytes| {
            output.extend_from_slice(&bytes);
            emitted.set(emitted.get() + 1);
            emitted.get() < accepted_records
        },
        &|| emitted.get() >= cancelled_after,
    )?;
    Ok((meta, output))
}

fn fixture_route(target: &str) -> crate::route::Route {
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(path, query)| (path, Some(query)));
    crate::route::parse(path, query).expect("valid fixture route")
}

fn record_kinds(bytes: &[u8]) -> Vec<String> {
    raw_ndjson_records(bytes)
        .iter()
        .map(|record| record["record"].as_str().expect("record kind").to_owned())
        .collect()
}

fn assert_api_error(
    error: &ApiError,
    status: StatusCode,
    code: &str,
    parameter: Option<&str>,
    message: &str,
) {
    assert_eq!(crate::api::api_error_status(error), status);
    assert_eq!(error.code(), code);
    assert_eq!(error.parameter(), parameter);
    assert_eq!(error.to_string(), message);
    assert!(!error.source_changed_during_read());
}

#[test]
fn finished_index_uses_its_checksum_for_the_etag() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    fixture.finish();
    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");

    let (meta, bytes) =
        query_response(&fixture, &target, usize::MAX, usize::MAX).expect("stream finished index");

    assert_eq!(meta.cache, CachePolicy::Immutable);
    let header = raw_ndjson_records(&bytes)
        .into_iter()
        .find(|record| record["record"] == "index")
        .expect("index header");
    let checksum = header["checksum"].as_str().expect("finished checksum");
    assert_eq!(
        meta.etag.as_deref(),
        Some(format!("W/\"{checksum}\"").as_str())
    );

    let not_modified = fixture.prepare(&target, meta.etag.as_deref());
    assert_eq!(not_modified.meta().status, StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.meta().etag, meta.etag);
    assert!(matches!(not_modified, Prepared::Empty(_)));
}

#[test]
fn active_index_has_no_etag_or_persisted_file() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");

    let (meta, bytes) =
        query_response(&fixture, &target, usize::MAX, usize::MAX).expect("stream active index");

    assert_eq!(meta.cache, CachePolicy::NoStore);
    assert_eq!(meta.etag, None);
    let header = raw_ndjson_records(&bytes)
        .into_iter()
        .find(|record| record["record"] == "index")
        .expect("index header");
    assert_eq!(header["checksum"], Value::Null);
    assert!(
        std::fs::read_dir(fixture.root())
            .expect("list active fixture root")
            .all(|entry| {
                entry.expect("active fixture entry").path().extension()
                    != Some(std::ffi::OsStr::new("idx"))
            }),
        "an active query must not persist an IDX"
    );
}

#[test]
fn index_returns_stable_errors_and_honors_stream_stops() {
    let mut fixture = Fixture::new();
    fixture.append_health();

    let missing = format!("/api/segments/{SEGMENT_ID}/sections/not_recorded/index");
    let error = query_response(&fixture, &missing, usize::MAX, usize::MAX)
        .expect_err("index rejects an unknown series");
    assert_api_error(
        &error,
        StatusCode::NOT_FOUND,
        "no_such_section",
        None,
        "no such logical section",
    );

    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    for (accepted_records, cancelled_after, expected) in [
        (usize::MAX, 0, &[][..]),
        (usize::MAX, 1, &["index"][..]),
        (1, usize::MAX, &["index"][..]),
        (2, usize::MAX, &["index", "layout"][..]),
    ] {
        let (meta, bytes) = query_response(&fixture, &target, accepted_records, cancelled_after)
            .expect("stop index stream");
        assert_eq!(meta.cache, CachePolicy::NoStore);
        assert_eq!(
            record_kinds(&bytes),
            expected,
            "accepted_records={accepted_records} cancelled_after={cancelled_after}"
        );
    }
}

#[test]
fn heatmap_preserves_cache_order_and_truncation() {
    for finished in [false, true] {
        let mut fixture = Fixture::new();
        fixture.append_resolved_diskstats(&[
            (100, 0, 10),
            (100, 1, 10),
            (100, 2, 10),
            (200, 0, 20),
            (200, 1, 40),
            (200, 2, 30),
        ]);
        if finished {
            fixture.finish();
        }
        let target =
            "/api/heatmap?from=100&to=200&section=os_diskstats&field=reads&columns=2&top=2";

        let (meta, bytes) =
            query_response(&fixture, target, usize::MAX, usize::MAX).expect("stream heatmap");

        assert_eq!(
            meta.cache,
            if finished {
                CachePolicy::Immutable
            } else {
                CachePolicy::NoStore
            }
        );
        assert_eq!(meta.etag.is_some(), finished);

        let records = raw_ndjson_records(&bytes);
        assert_eq!(records[0]["record"], "heatmap");
        assert_eq!(records[0]["summary"], "sum");
        assert_eq!(records[0]["entity_count"], 3);
        assert_eq!(records[0]["top"], 2);
        assert_eq!(records[0]["others_count"], 1);
        let rows = records
            .iter()
            .filter(|record| record["record"] == "heatmap_row")
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["identity"], serde_json::json!([8, 1]));
        assert_eq!(rows[1]["identity"], serde_json::json!([8, 2]));
        assert_eq!(rows[0]["total"], 30.0);
        assert_eq!(rows[1]["total"], 20.0);
        assert_eq!(records[records.len() - 2]["band"], "totals");
        assert_eq!(records[records.len() - 1]["band"], "others");
    }
}

#[test]
fn heatmap_uses_stable_group_tie_order_and_truncation() {
    let mut fixture = Fixture::new();
    fixture.append_resolved_diskstats(&[
        (100, 0, 0),
        (100, 1, 0),
        (100, 2, 0),
        (200, 0, 10),
        (200, 1, 10),
        (200, 2, 5),
    ]);
    fixture.finish();
    let target =
        "/api/heatmap?from=100&to=200&section=os_diskstats&field=reads&columns=2&top=1&group=minor";

    let (meta, bytes) =
        query_response(&fixture, target, usize::MAX, usize::MAX).expect("stream grouped heatmap");
    assert_eq!(meta.cache, CachePolicy::Immutable);
    assert!(meta.etag.is_some());

    let records = raw_ndjson_records(&bytes);
    assert_eq!(records[0]["group"], serde_json::json!(["minor"]));
    assert_eq!(records[0]["top"], 1);
    assert_eq!(records[0]["entity_count"], 3);
    assert_eq!(records[0]["others_count"], 2);
    assert_eq!(records[1]["identity"], serde_json::json!([0]));
    assert_eq!(records[1]["members"], 1);
    assert_eq!(records[1]["total"], 10.0);
    assert_eq!(records[records.len() - 1]["band"], "others");
    assert_eq!(records[records.len() - 1]["total"], 15.0);
}

#[test]
fn heatmap_returns_stable_errors_and_honors_stream_stops() {
    let mut fixture = Fixture::new();
    fixture.append_resolved_diskstats(&[(100, 0, 10), (100, 1, 10), (200, 0, 20), (200, 1, 40)]);
    fixture.finish();

    for (target, code, parameter, message) in [
        (
            "/api/heatmap?from=100&to=200&section=os_diskstats&field=missing&columns=2&top=2",
            "no_such_column",
            Some("missing"),
            "no such column \"missing\"",
        ),
        (
            "/api/heatmap?from=100&to=200&section=os_diskstats&field=reads&field=read_time_ms&columns=2&top=2",
            "mixed_units",
            Some("reads+read_time_ms"),
            "fields carry different units: reads+read_time_ms",
        ),
    ] {
        let error = query_response(&fixture, target, usize::MAX, usize::MAX)
            .expect_err("heatmap rejects the request");
        assert_api_error(&error, StatusCode::BAD_REQUEST, code, parameter, message);
    }

    let target = "/api/heatmap?from=100&to=200&section=os_diskstats&field=reads&columns=2&top=2";
    let error = query_response(&fixture, target, usize::MAX, 0)
        .expect_err("heatmap execution is cancelled");
    assert_api_error(
        &error,
        StatusCode::INTERNAL_SERVER_ERROR,
        "unreadable",
        None,
        "rankings[0]: request cancelled",
    );

    for (accepted_records, cancelled_after, expected) in [
        (1, usize::MAX, &["heatmap"][..]),
        (2, usize::MAX, &["heatmap", "heatmap_row"][..]),
        (
            3,
            usize::MAX,
            &["heatmap", "heatmap_row", "heatmap_row"][..],
        ),
        (usize::MAX, 1, &["heatmap"][..]),
        (usize::MAX, 2, &["heatmap", "heatmap_row"][..]),
        (
            usize::MAX,
            3,
            &[
                "heatmap",
                "heatmap_row",
                "heatmap_row",
                "heatmap_band",
                "heatmap_band",
            ][..],
        ),
    ] {
        let (meta, bytes) = query_response(&fixture, target, accepted_records, cancelled_after)
            .expect("stop heatmap stream");
        assert_eq!(meta.cache, CachePolicy::Immutable);
        assert_eq!(
            record_kinds(&bytes),
            expected,
            "accepted_records={accepted_records} cancelled_after={cancelled_after}"
        );
    }
}

#[test]
fn heatmap_validates_the_endpoint_before_opening_the_source() {
    let directory = tempfile::tempdir().expect("temporary parent");
    let missing = directory.path().join("missing-root");
    let target = "/api/heatmap?from=100&to=9223372036854775807&section=os_diskstats&field=reads&columns=2&top=2";
    let error = crate::api::prepare(&missing, SOURCES, fixture_route(target), None)
        .err()
        .expect("heatmap rejects an overflowing endpoint");
    assert_api_error(
        &error,
        StatusCode::BAD_REQUEST,
        "bad_filter",
        Some("to"),
        "invalid typed filter for \"to\"",
    );
}

#[test]
fn row_detail_rejects_a_malformed_ref_before_opening_storage() {
    let directory = tempfile::tempdir().expect("temporary parent");
    let missing = directory.path().join("missing-root");
    let error = crate::api::prepare(
        &missing,
        SOURCES,
        crate::route::Route::Recorded(kronika_api::Route::RowDetail("not+base64".to_owned())),
        None,
    )
    .err()
    .expect("row detail rejects a malformed reference");
    assert_api_error(
        &error,
        StatusCode::BAD_REQUEST,
        "bad_locator",
        None,
        "invalid detail_ref",
    );
}

#[test]
fn row_detail_http_bytes_and_cache_policy_stay_stable() {
    let mut fixture = Fixture::new();
    let at = SEGMENT_ID + 10;
    fixture.append_pgbouncer_event(at);
    fixture.finish();
    let events = format!(
        "/api/events?from={at}&to={}&source=pgbouncer_events&representation=occurrences&limit=1",
        at + 1
    );
    let (_meta, bytes) =
        query_response(&fixture, &events, usize::MAX, usize::MAX).expect("event occurrence");
    let occurrence = raw_ndjson_records(&bytes)
        .into_iter()
        .find(|record| record["record"] == "event_occurrence")
        .expect("event occurrence");
    let detail_ref = occurrence["detail_ref"].as_str().expect("detail ref");
    let target = format!("/api/row-detail?detail_ref={detail_ref}");

    let (meta, bytes) =
        query_response(&fixture, &target, usize::MAX, usize::MAX).expect("row detail");

    assert_eq!(meta.cache, CachePolicy::NoStore);
    assert_eq!(meta.etag, None);
    assert_eq!(
        bytes,
        format!(
            "{{\"at\":\"{at}\",\"fields\":{{\"database\":null,\"host\":null,\"level\":2,\"level_label\":\"warning\",\"source_file\":\"fixture\",\"text\":{{\"full_len\":\"7\",\"sha256\":null,\"stored_text\":\"fixture\",\"truncated\":false}},\"ts\":\"{at}\",\"username\":null}},\"record\":\"row_detail\",\"section\":\"pgbouncer_events\"}}\n"
        )
        .into_bytes()
    );
}

#[test]
fn events_preserve_group_and_occurrence_order_truncation_and_detail_refs() {
    let mut fixture = Fixture::new();
    let from = SEGMENT_ID + 10;
    let to_exclusive = from + 4;
    fixture.append_pgbouncer_event(from);
    fixture.append_log_error(from);
    fixture.append_log_error(from + 1);
    fixture.append_pgbouncer_event(from + 2);
    fixture.finish();

    for target in [
        format!(
            "/api/events?from={from}&to={to_exclusive}&source=pgbouncer_events&source=pg_log_errors&representation=groups&limit=1"
        ),
        format!(
            "/api/events?from={from}&to={to_exclusive}&source=pgbouncer_events&source=pg_log_errors&representation=groups&limit=10"
        ),
    ] {
        let (meta, bytes) =
            query_response(&fixture, &target, usize::MAX, usize::MAX).expect("stream event groups");
        assert_eq!(meta.cache, CachePolicy::Immutable);
        assert!(meta.etag.is_some());
        let records = raw_ndjson_records(&bytes);
        assert_eq!(records[0]["representation"], "groups");
        assert_eq!(records[0]["truncated"], target.ends_with("limit=1"));
        assert!(
            records
                .iter()
                .filter(|record| record["record"] == "event_group")
                .all(|record| record["detail_ref"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()))
        );
    }

    let target = format!(
        "/api/events?from={from}&to={to_exclusive}&source=pgbouncer_events&source=pg_log_errors&representation=occurrences&limit=3"
    );
    let (meta, bytes) = query_response(&fixture, &target, usize::MAX, usize::MAX)
        .expect("stream event occurrences");
    assert_eq!(meta.cache, CachePolicy::Immutable);
    assert!(meta.etag.is_some());
    let records = raw_ndjson_records(&bytes);
    assert_eq!(records[0]["representation"], "occurrences");
    assert_eq!(records[0]["truncated"], true);
    let occurrences = records
        .iter()
        .filter(|record| record["record"] == "event_occurrence")
        .collect::<Vec<_>>();
    assert_eq!(occurrences.len(), 3);
    assert_eq!(
        occurrences
            .iter()
            .map(|record| record["source"].as_str().expect("event source"))
            .collect::<Vec<_>>(),
        ["pgbouncer_events", "pg_log_errors", "pg_log_errors"]
    );
    assert!(occurrences.iter().all(|record| {
        record["detail_ref"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    }));
}

#[test]
fn events_keep_equivalent_rows_and_honor_stream_stops() {
    let mut fixture = Fixture::new();
    let from = SEGMENT_ID + 10;
    let to_exclusive = from + 3;
    fixture.append_log_error(from);
    fixture.append_log_error(from + 1);
    fixture.finish();
    let target = format!(
        "/api/events?from={from}&to={to_exclusive}&source=pg_log_errors&representation=occurrences&limit=10"
    );

    let error =
        query_response(&fixture, &target, usize::MAX, 0).expect_err("event execution is cancelled");
    assert_api_error(
        &error,
        StatusCode::INTERNAL_SERVER_ERROR,
        "cancelled",
        None,
        "request cancelled",
    );

    for cancelled_after in [1, 2] {
        let (meta, bytes) = query_response(&fixture, &target, usize::MAX, cancelled_after)
            .expect("cancel events after the header");
        assert_eq!(meta.cache, CachePolicy::Immutable);
        assert_eq!(
            record_kinds(&bytes),
            ["events", "event_occurrence", "event_occurrence"],
            "cancelled_after={cancelled_after}"
        );
    }

    for (accepted_records, expected) in [
        (1, &["events"][..]),
        (2, &["events", "event_occurrence"][..]),
    ] {
        let (meta, bytes) = query_response(&fixture, &target, accepted_records, usize::MAX)
            .expect("stop events stream");
        assert_eq!(meta.cache, CachePolicy::Immutable);
        assert_eq!(
            record_kinds(&bytes),
            expected,
            "accepted_records={accepted_records}"
        );
    }

    let mut duplicate = Fixture::new();
    duplicate.append_pgbouncer_event(from);
    duplicate.append_pgbouncer_event(from);
    duplicate.finish();
    let duplicate_target = format!(
        "/api/events?from={from}&to={to_exclusive}&source=pgbouncer_events&representation=occurrences&limit=10"
    );
    let (_meta, bytes) = query_response(&duplicate, &duplicate_target, usize::MAX, usize::MAX)
        .expect("stream equivalent events");
    let records = raw_ndjson_records(&bytes);
    let occurrences = records
        .iter()
        .filter(|record| record["record"] == "event_occurrence")
        .collect::<Vec<_>>();
    assert_eq!(occurrences.len(), 2);
    assert!(occurrences.iter().all(|record| {
        record["detail_ref"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    }));
    assert_eq!(
        event_detail(&duplicate, occurrences[0]),
        event_detail(&duplicate, occurrences[1]),
    );

    let group_target = format!(
        "/api/events?from={from}&to={to_exclusive}&source=pgbouncer_events&representation=groups&limit=10"
    );
    let (_meta, bytes) = query_response(&duplicate, &group_target, usize::MAX, usize::MAX)
        .expect("stream equivalent event group");
    let records = raw_ndjson_records(&bytes);
    let group = records
        .iter()
        .find(|record| record["record"] == "event_group")
        .expect("event group");
    assert_eq!(group["count"], 2.0);
    assert_eq!(group["label"], "fixture");
    assert_eq!(group["stat"]["database"], Value::Null);
    assert_eq!(group["stat"]["username"], Value::Null);
    assert_eq!(group["stat"]["host"], Value::Null);
    assert_eq!(group["stat"]["sourceFile"], "fixture");
    assert_eq!(
        event_detail(&duplicate, group)["section"],
        "pgbouncer_events"
    );
}

#[test]
fn hour_source_presence_uses_only_rows_inside_the_requested_window() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health_at(100, 0);
    fixture.finish();

    for (target, expected) in [
        ("/api/hour?from=100&to=199", true),
        ("/api/hour?from=200&to=300", false),
    ] {
        let records = stream(fixture.prepare_with_sources(target, None, SOURCE_OS))
            .expect("bounded hour response");
        let family = records
            .iter()
            .find(|record| record["record"] == "catalog")
            .and_then(|record| record["source_families"].as_array())
            .and_then(|families| {
                families
                    .iter()
                    .find(|family| family["name"] == "postgresql")
            })
            .expect("PostgreSQL source family");
        assert_eq!(family["configured"], false);
        assert_eq!(family["present"], expected);
        assert_eq!(family["metrics_present"], expected);
    }
}

#[test]
fn hour_source_presence_folds_contained_inventory_and_scans_partial_rows() {
    let mut contained = Fixture::new();
    contained.append_log_error(100);
    contained.finish();
    let records = stream(contained.prepare("/api/hour?from=100&to=100&part=base", None))
        .expect("contained source inventory");
    assert_eq!(source_flags(&records, "postgresql"), (true, false));

    let mut partial = Fixture::new();
    partial.append_log_error(99);
    partial.append_log_error(100);
    partial.finish();
    let records = stream(partial.prepare("/api/hour?from=100&to=100&part=base", None))
        .expect("partial positive source");
    assert_eq!(source_flags(&records, "postgresql"), (true, false));

    let mut straddling = Fixture::new();
    straddling.append_log_error(99);
    straddling.append_diskstats(&[(100, 0, 1)]);
    straddling.finish();
    let records = stream(straddling.prepare("/api/hour?from=100&to=100&part=base", None))
        .expect("straddling source inventory");
    assert_eq!(source_flags(&records, "os"), (true, true));
    assert_eq!(source_flags(&records, "postgresql"), (false, false));
}

#[test]
fn postgres_log_rows_do_not_claim_selected_hour_metrics() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let records =
        stream(fixture.prepare_with_sources("/api/hour?from=100&to=199", None, SOURCE_OS))
            .expect("log-only hour response");
    let family = records
        .iter()
        .find(|record| record["record"] == "catalog")
        .and_then(|record| record["source_families"].as_array())
        .and_then(|families| {
            families
                .iter()
                .find(|family| family["name"] == "postgresql")
        })
        .expect("PostgreSQL source family");
    assert_eq!(family["configured"], false);
    assert_eq!(family["present"], true);
    assert_eq!(family["metrics_present"], false);
}

#[tokio::test]
async fn weak_index_etag_revalidates_both_representations_without_a_304_body() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    fixture.finish();
    let resource = format!("/api/segments/{SEGMENT_ID}/sections/health/index");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response
        .headers()
        .get(ETAG)
        .expect("ETag")
        .to_str()
        .expect("ASCII ETag")
        .to_owned();
    assert!(etag.starts_with("W/\""));
    assert_eq!(
        response.headers().get(VARY),
        Some(&HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
    );
    assert!(
        !response
            .into_body()
            .collect()
            .await
            .expect("index body")
            .to_bytes()
            .is_empty()
    );

    let strong = etag.strip_prefix("W/").expect("weak validator");
    for offered in [format!("\"stale\", {strong}"), "*".to_owned()] {
        let prepared = fixture.prepare(&resource, Some(&offered));
        let response =
            crate::blocking_stream(move || Ok(prepared), accepted("gzip, identity;q=0")).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{offered}");
        assert_eq!(
            response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok()),
            Some(etag.as_str())
        );
        assert_eq!(
            response.headers().get(VARY),
            Some(&HeaderValue::from_static(
                "Authorization, Cookie, Accept-Encoding"
            ))
        );
        assert!(!response.headers().contains_key(CONTENT_ENCODING));
        assert!(
            response
                .into_body()
                .collect()
                .await
                .expect("304 body")
                .to_bytes()
                .is_empty()
        );
    }
}

#[test]
fn health_is_streamed_as_an_ordinary_history_series_from_real_sections() {
    let mut fixture = Fixture::new();
    fixture.append_health();

    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/history?field=health");
    let records = stream(fixture.prepare(&target, None)).expect("health history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["timestamp"], "100");
    assert_eq!(rows[0]["values"], serde_json::json!([null]));
    assert_eq!(rows[1]["timestamp"], "200");
    assert_eq!(rows[1]["values"], serde_json::json!([50]));

    let index_target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    let index = fixture.prepare(&index_target, None);
    assert_eq!(index.meta().cache, CachePolicy::NoStore);
    let index = stream(index).expect("health index");
    assert!(index.iter().any(|record| {
        record["record"] == "point"
            && record["series"] == "os_health"
            && record["type_id"] == "0"
            && record["value"] == 50
    }));
}

#[test]
fn postgres_health_counts_every_active_backend_and_adds_its_penalty() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(5);

    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    let records = stream(fixture.prepare(&target, None)).expect("health index");
    let points = |series: &str| {
        records
            .iter()
            .filter(|record| record["record"] == "point" && record["series"] == series)
            .collect::<Vec<_>>()
    };
    assert_eq!(points("postgres_health")[0]["value"], 80);
    assert_eq!(points("overall_health")[1]["value"], 30);
    assert_eq!(points("active_backends").len(), 0);
}

#[test]
fn a_snapshot_answers_for_the_sections_that_are_there() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(3);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_activity&section=os_diskstats"
    );
    let prepared = fixture.prepare(&target, None);
    assert_eq!(prepared.meta().status, StatusCode::OK);
    let records = stream(prepared).expect("snapshot body");
    let sections = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| record["layout"]["logical_name"].clone())
        .collect::<Vec<_>>();
    assert_eq!(sections, [serde_json::json!("pg_stat_activity")]);
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|row| row["timestamp"] == "150"));
}

#[test]
fn a_multi_section_snapshot_applies_the_shared_projection_per_section() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(3);
    fixture.append_diskstats(&[(200, 0, 7)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_activity&section=os_diskstats&section=not_recorded&field=pid&field=minor"
    );
    let records = stream(fixture.prepare(&target, None)).expect("projected snapshot");
    let layouts = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| {
            (
                record["layout"]["logical_name"].clone(),
                record["layout"]["columns"]
                    .as_array()
                    .expect("layout columns")
                    .iter()
                    .map(|column| column["name"].clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        layouts,
        [
            (
                serde_json::json!("pg_stat_activity"),
                vec![serde_json::json!("pid")]
            ),
            (
                serde_json::json!("os_diskstats"),
                vec![serde_json::json!("minor")]
            ),
        ]
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["values"], serde_json::json!([0]));
    assert_eq!(rows[4]["values"], serde_json::json!([0]));
}

#[test]
fn a_multi_section_snapshot_keeps_partitioned_and_shared_predecessors_independent() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(&[(100, 1, 77, 10), (200, 1, 77, 20)], &[], &[]);
    fixture.append_diskstats(&[(200, 0, 7)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&section=os_diskstats&field=datid&field=minor"
    );
    let records = stream(fixture.prepare(&target, None)).expect("mixed snapshot");
    let layouts = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| record["layout"]["logical_name"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        layouts,
        [
            serde_json::json!("pg_stat_user_tables"),
            serde_json::json!("os_diskstats"),
        ]
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["values"], serde_json::json!([1]));
    assert_eq!(rows[1]["values"], serde_json::json!([0]));
}

#[test]
fn snapshot_rows_align_positional_values_with_layout_columns() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..32)
            .map(|minor| (200, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=major&field=minor&field=scope&field=io_in_progress"
    );
    let prepared = fixture.prepare(&target, None);
    let mut body = Vec::new();
    prepared
        .stream(
            &mut |record| {
                body.extend_from_slice(&record);
                true
            },
            &|| false,
        )
        .expect("snapshot body");
    let text = std::str::from_utf8(&body).expect("UTF-8 snapshot");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON record"))
        .collect::<Vec<_>>();
    let layout = records
        .iter()
        .find(|record| record["record"] == "layout")
        .expect("projected layout");
    assert_eq!(
        layout["layout"]["columns"]
            .as_array()
            .expect("layout columns")
            .iter()
            .map(|column| column["name"].clone())
            .collect::<Vec<_>>(),
        ["major", "minor", "scope", "io_in_progress"].map(Value::from)
    );
    assert_eq!(
        layout["layout"]["identity"],
        serde_json::json!(["major", "minor"])
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 32);
    for (minor, row) in rows.iter().enumerate() {
        assert_eq!(row["values"], serde_json::json!([8, minor, 0, "0"]));
        assert!(row["values"].as_object().is_none());
    }
}

#[test]
fn an_hour_carries_its_segments_and_its_line_in_one_response() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    let active = fixture.prepare("/api/hour?from=0&to=1000", None);
    assert_eq!(active.meta().cache, CachePolicy::NoStore);
    fixture.finish();

    let prepared = fixture.prepare("/api/hour?from=0&to=1000", None);
    assert_eq!(prepared.meta().status, StatusCode::OK);
    assert_eq!(prepared.meta().cache, CachePolicy::Immutable);
    let records = stream(prepared).expect("hour body");
    let kinds = records
        .iter()
        .map(|record| record["record"].as_str().expect("record kind"))
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"catalog"), "{kinds:?}");
    assert!(kinds.contains(&"finished_segment"), "{kinds:?}");
    assert!(kinds.contains(&"index"), "{kinds:?}");
    let points = records
        .iter()
        .filter(|record| record["record"] == "point")
        .collect::<Vec<_>>();
    assert!(!points.is_empty());
    assert!(points.iter().all(|point| point["series"] != Value::Null));
    let segment = records
        .iter()
        .find(|record| record["record"] == "index")
        .expect("index header");
    assert_eq!(segment["segment"]["id"], SEGMENT_ID.to_string());
    assert_eq!(segment["logical_name"], "health");
}

#[test]
fn finished_browser_resources_are_immutable_and_revalidate_without_a_body() {
    let mut fixture = Fixture::new();
    fixture.append_resolved_diskstats(&[(100, 0, 1), (200, 0, 2)]);
    fixture.finish();

    for target in browser_resource_targets() {
        let initial = fixture.prepare(&target, None);
        let meta = initial.meta();
        assert_eq!(meta.status, StatusCode::OK, "{target}");
        assert_eq!(meta.cache, CachePolicy::Immutable, "{target}");
        let etag = meta.etag.expect("finished browser resource ETag");
        assert!(etag.starts_with("W/\""), "{target}");
        assert!(!stream(initial).expect("finished response body").is_empty());

        let matching = fixture.prepare(&target, Some(&etag));
        assert_eq!(matching.meta().status, StatusCode::NOT_MODIFIED, "{target}");
        assert_eq!(matching.meta().cache, CachePolicy::Immutable, "{target}");
        assert_eq!(
            matching.meta().etag.as_deref(),
            Some(etag.as_str()),
            "{target}"
        );
        assert!(matches!(matching, Prepared::Empty(_)), "{target}");
    }
}

#[test]
fn aggregate_reads_with_catalog_warnings_are_not_immutable() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (200, 0, 2)]);
    fixture.finish();
    fixture.add_foreign_entry();

    for target in browser_resource_targets() {
        let prepared = fixture.prepare(&target, None);
        assert_eq!(prepared.meta().cache, CachePolicy::Revalidate, "{target}");
        assert_eq!(prepared.meta().etag, None, "{target}");
    }
}

#[test]
fn a_validator_identifies_the_request_it_was_issued_for() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (200, 0, 2)]);
    fixture.finish();

    for (issued, other) in browser_resource_targets()
        .into_iter()
        .zip(neighbouring_resource_targets())
    {
        let etag = fixture
            .prepare(&issued, None)
            .meta()
            .etag
            .expect("finished browser resource ETag");
        let neighbour = fixture.prepare(&other, None);
        assert_ne!(
            neighbour.meta().etag.as_deref(),
            Some(etag.as_str()),
            "{other} reused the validator of {issued}"
        );
        assert_eq!(
            fixture.prepare(&other, Some(&etag)).meta().status,
            StatusCode::OK,
            "{other} answered 304 to the validator of {issued}"
        );
    }
}

#[test]
fn a_validator_stops_matching_once_the_window_gains_a_segment() {
    let target = "/api/hour?from=100&to=200";
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 1);
    let etag = fixture
        .prepare(target, None)
        .meta()
        .etag
        .expect("finished hour ETag");
    assert_eq!(
        fixture.prepare(target, Some(&etag)).meta().status,
        StatusCode::NOT_MODIFIED
    );

    fixture.append_diskstats(&[(150, 1, 5)]);
    fixture.finish();
    assert_eq!(
        fixture.prepare(target, Some(&etag)).meta().status,
        StatusCode::OK
    );
}

#[test]
fn matching_finished_snapshot_etag_returns_not_modified() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(
        &[(10_000_000, 1, 77, 10), (30_000_000, 1, 77, 30)],
        &[],
        &[],
    );
    fixture.finish();
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_tables&field=seq_scan"
    );

    let initial = fixture.prepare(&target, None);
    let etag = initial.meta().etag.expect("finished snapshot ETag");

    let matching = fixture.prepare(&target, Some(&etag));
    assert_eq!(matching.meta().status, StatusCode::NOT_MODIFIED);
}

#[test]
fn active_browser_resources_are_not_reusable() {
    let mut fixture = Fixture::new();
    fixture.append_resolved_diskstats(&[(100, 0, 1), (200, 0, 2)]);

    for target in browser_resource_targets() {
        let prepared = fixture.prepare(&target, None);
        assert_eq!(prepared.meta().status, StatusCode::OK, "{target}");
        assert_eq!(prepared.meta().cache, CachePolicy::NoStore, "{target}");
        assert_eq!(prepared.meta().etag, None, "{target}");
        assert!(!stream(prepared).expect("active response body").is_empty());
    }
}

#[test]
fn empty_finished_hour_series_has_no_validator() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (200, 0, 2)]);
    fixture.finish();

    let prepared = fixture.prepare(
        "/api/hour?from=300&to=400&section=os_diskstats&field=reads",
        None,
    );
    assert_eq!(prepared.meta().cache, CachePolicy::Revalidate);
    assert_eq!(prepared.meta().etag, None);
}

#[test]
fn empty_finished_heatmap_has_no_validator() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (200, 0, 2)]);
    fixture.finish();

    let prepared = fixture.prepare(
        "/api/heatmap?from=300&to=400&section=os_diskstats&field=reads&columns=1&top=1",
        None,
    );
    assert_eq!(prepared.meta().cache, CachePolicy::Revalidate);
    assert_eq!(prepared.meta().etag, None);
}

// Each entry differs from its browser_resource_targets peer in one parameter.
fn neighbouring_resource_targets() -> [String; 3] {
    [
        "/api/hour?from=100&to=150".to_owned(),
        format!("/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=reads"),
        "/api/heatmap?from=100&to=200&section=os_diskstats&field=reads&columns=2&top=1".to_owned(),
    ]
}

fn browser_resource_targets() -> [String; 3] {
    [
        "/api/hour?from=100&to=200".to_owned(),
        format!("/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=reads"),
        "/api/heatmap?from=100&to=200&section=os_diskstats&field=reads&columns=1&top=1".to_owned(),
    ]
}

#[test]
fn an_hour_lists_all_available_hours_but_only_selected_segments() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 3_600_000_000);
    fixture.append_diskstats(&[(3_600_000_100, 0, 2)]);
    fixture.finish();

    let records =
        stream(fixture.prepare("/api/hour?from=100&to=200", None)).expect("selected hour response");
    let hour = records
        .iter()
        .find(|record| record["record"] == "hour")
        .expect("hour header");
    assert_eq!(
        hour["available_hours"],
        serde_json::json!(["0", "3600000000"])
    );
    let segments = records
        .iter()
        .filter(|record| record["record"] == "finished_segment")
        .map(|record| record["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(segments, [SEGMENT_ID.to_string()].map(Value::from));
}

#[test]
fn an_hour_keeps_generic_rows_and_lanes_inside_its_inclusive_window() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(99, 0, 9), (100, 0, 0), (200, 0, 2), (201, 0, 1)]);

    let series = stream(fixture.prepare(
        "/api/hour?from=100&to=200&section=os_diskstats&field=reads&field=discards",
        None,
    ))
    .expect("bounded generic series");
    let rows = row_records(&series);
    assert_eq!(
        rows.iter()
            .map(|row| row["timestamp"].clone())
            .collect::<Vec<_>>(),
        [serde_json::json!("100"), serde_json::json!("200")]
    );
    assert_eq!(rows[0]["values"], serde_json::json!(["0", null]));
    assert_eq!(rows[1]["values"], serde_json::json!(["2", null]));

    let hour =
        stream(fixture.prepare("/api/hour?from=100&to=200", None)).expect("bounded timeline lanes");
    let lanes = hour
        .iter()
        .filter(|record| record["record"] == "lane")
        .collect::<Vec<_>>();
    assert!(!lanes.is_empty());
    assert!(
        lanes
            .iter()
            .all(|lane| matches!(lane["ts"].as_str(), Some("100" | "200")))
    );
    for timestamp in ["100", "200"] {
        assert!(lanes.iter().any(|lane| lane["ts"] == timestamp));
    }
}

#[test]
fn hour_base_and_pinned_lanes_compose_the_legacy_response_once() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health_with_interval(100, 5, 17);
    fixture.append_diskstats(&[(100, 0, 0), (200, 0, 2)]);
    let second = SEGMENT_ID + 1;
    fixture.finish_and_continue(second);
    fixture.append_postgres_health_with_interval(300, 4, 19);
    fixture.append_diskstats(&[(300, 0, 4), (400, 0, 8)]);
    fixture.finish();

    let legacy_prepared = fixture.prepare("/api/hour?from=100&to=400", None);
    let legacy_etag = legacy_prepared.meta().etag.expect("legacy ETag");
    let legacy = stream(legacy_prepared).expect("legacy combined hour");

    let base_prepared = fixture.prepare("/api/hour?from=100&to=400&part=base", None);
    assert_eq!(base_prepared.meta().cache, CachePolicy::Revalidate);
    assert_eq!(base_prepared.meta().etag, None);
    let base = stream(base_prepared).expect("lightweight hour base");
    assert!(base.iter().all(|record| record["record"] != "lane"));
    assert!(base.iter().any(|record| record["record"] == "catalog"));
    assert!(base.iter().any(|record| record["record"] == "point"));

    let lanes_prepared = fixture.prepare(
        &format!("/api/hour?from=100&to=400&part=lanes&segments={SEGMENT_ID},{second}"),
        None,
    );
    assert_eq!(lanes_prepared.meta().cache, CachePolicy::Immutable);
    let lanes_etag = lanes_prepared.meta().etag.expect("lanes ETag");
    assert_ne!(lanes_etag, legacy_etag);
    let lanes = stream(lanes_prepared).expect("finished pinned lanes");
    assert!(
        lanes
            .iter()
            .all(|record| { matches!(record["record"].as_str(), Some("lane_context" | "lane")) })
    );
    let contexts = lanes
        .iter()
        .filter(|record| record["record"] == "lane_context")
        .collect::<Vec<_>>();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(contexts[0]["postgresql_interval_seconds"], "17");
    assert_eq!(contexts[1]["segment_id"], second.to_string());
    assert_eq!(contexts[1]["postgresql_interval_seconds"], "19");

    assert_eq!(
        base,
        legacy
            .iter()
            .filter(|record| record["record"] != "lane")
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        lanes
            .iter()
            .filter(|record| record["record"] == "lane")
            .collect::<Vec<_>>(),
        legacy
            .iter()
            .filter(|record| record["record"] == "lane")
            .collect::<Vec<_>>()
    );
}

#[test]
fn active_hour_lanes_require_the_base_segment_set_and_wal_position() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health_with_interval(100, 5, 17);
    let position = fixture.position();
    let base_prepared = fixture.prepare("/api/hour?from=100&to=200&part=base", None);
    assert_eq!(base_prepared.meta().cache, CachePolicy::NoStore);
    let base = stream(base_prepared).expect("active hour base");
    let active = base
        .iter()
        .find(|record| record["record"] == "active_segment")
        .expect("active segment bound");
    assert_eq!(active["id"], SEGMENT_ID.to_string());
    assert_eq!(active["cursor"]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(active["cursor"]["wal_position"], position.to_string());
    let target = format!(
        "/api/hour?from=100&to=200&part=lanes&segments={SEGMENT_ID}&active={SEGMENT_ID},{position}"
    );

    fixture.append_diskstats(&[(150, 0, 1)]);
    let pinned_prepared = prepare_result(&fixture, &target).expect("valid active pin");
    assert_eq!(pinned_prepared.meta().cache, CachePolicy::NoStore);
    let pinned = stream(pinned_prepared).expect("pinned active lanes");
    assert!(pinned.iter().all(|record| {
        record["record"] != "lane" || record["lane"] != "disk_busy" || record["ts"] != "150"
    }));
    assert!(matches!(
        prepare_result(
            &fixture,
            &format!("/api/hour?from=100&to=200&part=lanes&segments={SEGMENT_ID}")
        ),
        Err(ApiError::BadCursor)
    ));
    assert!(matches!(
        prepare_result(
            &fixture,
            &format!(
                "/api/hour?from=100&to=200&part=lanes&segments={SEGMENT_ID}&active={SEGMENT_ID},{}",
                u64::MAX
            )
        ),
        Err(ApiError::BadCursor)
    ));

    let next = SEGMENT_ID + 1;
    fixture.finish_and_continue(next);
    fixture.append_diskstats(&[(175, 0, 2)]);
    assert!(matches!(
        prepare_result(&fixture, &target),
        Err(ApiError::BadCursor)
    ));
}

#[test]
fn history_resolves_dictionary_values_across_bounded_chunks() {
    let mut fixture = Fixture::new();
    let source = (0_i32..1_025)
        .map(|minor| (minor, "nvme0n1"))
        .collect::<Vec<_>>();
    fixture.append_named_diskstats(&source);
    fixture.finish();

    let records = stream(fixture.prepare(
        &format!("/api/segments/{SEGMENT_ID}/sections/os_diskstats/history?field=device"),
        None,
    ))
    .expect("bounded history response");
    let rows = row_records(&records);
    assert_eq!(rows.len(), source.len());
    assert!(
        rows.iter()
            .all(|row| row["values"] == serde_json::json!(["nvme0n1"]))
    );
}

#[test]
fn an_hour_reads_one_index_resource_per_segment_without_statistical_noise() {
    const STEP: i64 = 5 * 60 * 1_000_000;

    let mut fixture = Fixture::new();
    let process_prior = (0_i64..6)
        .map(|at| (SEGMENT_ID + at * STEP, Some(at * 300)))
        .collect::<Vec<_>>();
    let statement_prior = (0_i32..6)
        .map(|at| {
            (
                SEGMENT_ID + i64::from(at) * STEP,
                i64::from(at),
                f64::from(at) * 100.0,
            )
        })
        .collect::<Vec<_>>();
    fixture.append_finding_rows(&process_prior, &statement_prior);

    let current_at = SEGMENT_ID + 6 * STEP;
    fixture.finish_and_continue(current_at);
    fixture.append_finding_rows(&[(current_at, Some(301_500))], &[(current_at, 6, 10_500.0)]);
    fixture.append_postgres_health_at(current_at, 5);
    fixture.append_log_error(current_at + 60);
    fixture.append_log_temp_file(current_at + 61);
    fixture.finish();

    let records = stream(fixture.prepare(
        &format!(
            "/api/hour?from={SEGMENT_ID}&to={}",
            SEGMENT_ID + 3_600_000_000 - 1
        ),
        None,
    ))
    .expect("hour with findings");
    let index_segments = records
        .iter()
        .filter(|record| record["record"] == "index")
        .map(|record| record["segment"]["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        index_segments,
        [SEGMENT_ID.to_string(), current_at.to_string()].map(Value::from)
    );

    let findings = records
        .iter()
        .filter(|record| record["record"] == "finding")
        .collect::<Vec<_>>();
    for (logical_name, kind) in [
        ("pg_stat_activity", "known_bad"),
        ("pg_log_errors", "event"),
    ] {
        assert!(
            findings.iter().any(|finding| {
                finding["logical_name"] == logical_name && finding["kind"] == kind
            })
        );
    }
    let error = findings
        .iter()
        .find(|finding| finding["logical_name"] == "pg_log_errors")
        .expect("error event");
    assert_eq!(error["category"], 8);
    assert!(findings.iter().all(|finding| {
        finding["logical_name"] != "os_process"
            && finding["logical_name"] != "pg_stat_statements"
            && finding["kind"] != "spike"
    }));
    assert!(records.iter().all(|record| {
        !matches!(record["record"].as_str(), Some("findings" | "finding"))
            || record["type_id"] != "2007001"
    }));
}

#[test]
fn an_hour_filters_first_and_last_segment_findings_to_its_exact_bounds() {
    let from = SEGMENT_ID + 100;
    let to = SEGMENT_ID + 200;
    let mut fixture = Fixture::new();
    fixture.append_log_error(from - 1);
    fixture.append_log_error(from);
    fixture.finish_and_continue(to);
    fixture.append_log_error(to);
    fixture.append_log_error(to + 1);
    fixture.finish();

    let records = stream(fixture.prepare(&format!("/api/hour?from={from}&to={to}"), None))
        .expect("bounded hour");
    let summaries = records
        .iter()
        .filter(|record| record["record"] == "findings")
        .collect::<Vec<_>>();
    assert_eq!(
        summaries
            .iter()
            .map(|record| record["total_hits"].as_u64().expect("finding count"))
            .sum::<u64>(),
        2
    );
    assert!(summaries.iter().all(|record| record["truncated"] == false));
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"] == "finding")
            .map(|record| record["ts"].clone())
            .collect::<Vec<_>>(),
        [from.to_string(), to.to_string()].map(Value::from)
    );
}

#[test]
fn process_and_statement_rows_remain_available_without_findings() {
    let mut fixture = Fixture::new();
    fixture.append_finding_rows(&[(100, Some(4_096))], &[(100, 1, 2.5)]);
    fixture.finish();

    for target in [
        format!("/api/segments/{SEGMENT_ID}/sections/os_process/history?field=ts&field=read_bytes"),
        format!(
            "/api/segments/{SEGMENT_ID}/sections/os_process/rows?page_size=10&order=asc&field=ts&field=read_bytes"
        ),
        format!(
            "/api/segments/{SEGMENT_ID}/sections/pg_stat_statements/history?field=ts&field=calls&field=total_exec_time"
        ),
        format!(
            "/api/segments/{SEGMENT_ID}/sections/pg_stat_statements/rows?page_size=10&order=asc&field=ts&field=calls&field=total_exec_time"
        ),
    ] {
        let records = stream(fixture.prepare(&target, None)).expect("raw metric read");
        assert_eq!(row_records(&records).len(), 1, "{target}");
        assert!(
            records.iter().all(|record| {
                !matches!(record["record"].as_str(), Some("findings" | "finding"))
            })
        );
    }
}

#[test]
fn process_summary_series_uses_the_complete_set_and_previous_segment() {
    let mut fixture = Fixture::new();
    fixture.append_process_summary_snapshot(1_000_000, 1_000, None, 900_000, 0..3, None);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_process_summary_snapshot(
        6_000_000,
        1_100,
        Some(0),
        5_500_000,
        0..10,
        Some((6_500_000, 0..205)),
    );
    fixture.finish();

    let fields = [
        "processes",
        "threads",
        "runnable",
        "postgresql",
        "user_cores",
        "system_cores",
        "run_delay_ms_per_second",
        "context_switches_per_second",
        "resident_kib",
        "virtual_kib",
        "swap_kib",
        "major_faults_per_second",
        "read_bytes_per_second",
        "write_bytes_per_second",
        "read_calls_per_second",
        "write_calls_per_second",
    ];
    let query = fields
        .iter()
        .map(|field| format!("field={field}"))
        .collect::<Vec<_>>()
        .join("&");
    let records = stream(fixture.prepare(
        &format!("/api/hour?from=6000000&to=6000000&section=os_process_summary&{query}"),
        None,
    ))
    .expect("process summary history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["timestamp"], "6000000");
    let values = rows[0]["values"].as_array().expect("summary values");
    assert_eq!(values.len(), fields.len());
    assert_eq!(values[0], 205.0, "the server does not use a 200-row page");
    assert_eq!(values[1], 410.0);
    assert_eq!(values[2], 103.0);
    assert_eq!(values[3], 10.0, "a future activity snapshot is not joined");
    assert_eq!(values[4], 40.8, "PID reuse has no predecessor");
    assert_eq!(values[5], 81.6);
    assert_eq!(values[6], 4_080.0);
    assert_eq!(values[7], 12_240.0);
    assert_eq!(values[8], 22_960.0);
    assert_eq!(values[9], 25_010.0);
    assert_eq!(values[10], 204.0);
    assert_eq!(values[11], 4_080.0);
    assert_eq!(values[12], 4_080.0);
    assert_eq!(values[13], Value::Null, "all unavailable values stay null");
    assert_eq!(values[14], 4_080.0);
    assert_eq!(values[15], 8_160.0);
}

#[test]
fn process_snapshot_counter_history_rejects_pid_reuse() {
    let mut fixture = Fixture::new();
    fixture.append_process_summary_snapshot(1_000_000, 1_000, None, 900_000, 0..0, None);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_process_summary_snapshot(6_000_000, 1_100, Some(0), 5_500_000, 0..0, None);
    fixture.finish();

    let records = stream(fixture.prepare(
        &format!("/api/segments/{current_segment}/snapshot?at=6000000&section=os_process&field=pid&field=utime&where.pid=0"),
        None,
    ))
    .expect("PID-scoped process snapshot");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([0, null]));
}

#[test]
fn process_summary_rates_across_active_wal_parts() {
    let mut fixture = Fixture::new();
    fixture.append_process_summary_snapshot(1_000_000, 1_000, None, 900_000, 0..3, None);
    fixture.append_process_summary_snapshot(6_000_000, 1_100, None, 5_500_000, 0..3, None);

    let records = stream(fixture.prepare(
        "/api/hour?from=6000000&to=6000000&section=os_process_summary&field=user_cores&field=read_bytes_per_second",
        None,
    ))
    .expect("active process summary history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([41.0, 4_100.0]));
}

#[test]
fn temporary_file_rows_remain_available_through_generic_reads() {
    let mut fixture = Fixture::new();
    fixture.append_log_temp_file(100);
    fixture.finish();

    for resource in ["rows?page_size=10&order=asc&", "history?"] {
        let records = stream(fixture.prepare(
            &format!(
                "/api/segments/{SEGMENT_ID}/sections/pg_log_temp_files/{resource}field=ts&field=system_identifier&field=source_file&field=path&field=size_bytes&field=statement"
            ),
            None,
        ))
        .expect("generic temporary-file read");
        let layout = records
            .iter()
            .find(|record| record["record"] == "layout")
            .expect("temporary-file layout");
        assert_eq!(layout["layout"]["type_id"], "2007001");
        let rows = row_records(&records);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["values"],
            serde_json::json!([
                "100",
                "42",
                "postgresql.log",
                "base/pgsql_tmp/pgsql_tmp42.0",
                "1048576",
                "select fixture spill"
            ])
        );
    }
}

#[test]
fn snapshot_cache_policy_tracks_active_and_finished_inputs() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 0, 9)]);
    let resource =
        format!("/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=reads");

    let active = fixture.prepare(&resource, None);
    assert_eq!(active.meta().cache, CachePolicy::NoStore);

    fixture.finish();
    let finished = fixture.prepare(&resource, None);
    assert_eq!(finished.meta().cache, CachePolicy::Immutable);
}

#[test]
fn a_snapshot_orders_by_a_column_and_returns_one_page() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[
        (100, 0, 1),
        (100, 1, 9),
        (100, 2, 5),
        (200, 0, 2),
        (200, 1, 30),
        (200, 2, 11),
    ]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=major&field=minor&by=minor&page_size=2"
    );
    let records = stream(fixture.prepare(&target, None)).expect("ordered snapshot");
    let minors = records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|record| record["values"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        minors,
        [serde_json::json!([8, 2]), serde_json::json!([8, 1])]
    );
    let selection = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("snapshot row counts");
    assert_eq!(selection["eligible"], "3");
    assert_eq!(selection["returned"], "2");
    assert_eq!(selection["truncated"], true);
    assert_eq!(selection["page_size"], 2);
    assert_eq!(selection["has_more"], true);
    assert!(selection["next_cursor"].is_string());
    assert_eq!(selection["order_by"], serde_json::json!(["minor"]));
    assert_eq!(selection["order_direction"], "desc");
    assert_eq!(selection["from"], "100");
    assert_eq!(selection["to"], "200");
}

#[test]
fn a_snapshot_orders_stored_text_lexicographically_and_breaks_ties_by_ordinal() {
    let mut fixture = Fixture::new();
    fixture.append_named_diskstats(&[(0, "beta"), (1, "alpha"), (2, "gamma"), (3, "gamma")]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&field=device&by=device&page_size=3"
    );
    let records = stream(fixture.prepare(&target, None)).expect("text-ranked snapshot");
    let values = row_records(&records)
        .into_iter()
        .map(|row| row["values"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            serde_json::json!([2, "gamma"]),
            serde_json::json!([3, "gamma"]),
            serde_json::json!([0, "beta"]),
        ]
    );
    let selection = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("snapshot row counts");
    assert_eq!(selection["from"], Value::Null);
    assert_eq!(selection["to"], "100");
}

#[test]
fn a_snapshot_ranks_counter_rates_before_slicing_a_page() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[
        (100, 0, 1),
        (100, 1, 9),
        (100, 2, 5),
        (200, 0, 2),
        (200, 1, 30),
        (200, 2, 11),
    ]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&by=reads&page_size=2"
    );
    let records = stream(fixture.prepare(&target, None)).expect("counter-ranked snapshot");
    let values = row_records(&records)
        .into_iter()
        .map(|row| row["values"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            serde_json::json!([1, 210_000.0]),
            serde_json::json!([2, 60_000.0])
        ]
    );
}

#[test]
fn statement_pages_rank_each_displayed_composite_before_slicing() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    for (token, semantic) in [
        ("derived.mean_exec_ms_per_call", "mean_exec_ms_per_call"),
        ("derived.rows_per_call", "rows_per_call"),
        ("derived.blocks_per_call", "blocks_per_call"),
        ("derived.hit_pct", "hit_pct"),
        ("derived.wal_per_call", "wal_per_call"),
        ("derived.plan_time_pct", "plan_time_pct"),
        ("derived.cv", "cv"),
    ] {
        let target = format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=queryid&by={token}&page_size=1"
        );
        let records = stream(fixture.prepare(&target, None)).expect("composite statement page");
        assert_eq!(row_records(&records)[0]["values"], serde_json::json!(["2"]));
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("composite statement trailer");
        assert_eq!(page["eligible"], "3");
        assert_eq!(page["has_more"], true);
        assert_eq!(page["order_by"], serde_json::json!([semantic]));
        assert_eq!(page["order_direction"], "desc");
    }
}

#[test]
fn plan_pages_rank_each_displayed_composite_before_slicing() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_plans();
    fixture.finish();

    for (token, semantic) in [
        ("derived.mean_exec_ms_per_call", "mean_exec_ms_per_call"),
        ("derived.rows_per_call", "rows_per_call"),
        ("derived.blocks_per_call", "blocks_per_call"),
        ("derived.hit_pct", "hit_pct"),
    ] {
        let target = format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_store_plans&field=queryid&by={token}&page_size=1"
        );
        let records = stream(fixture.prepare(&target, None)).expect("composite plan page");
        assert_eq!(row_records(&records)[0]["values"], serde_json::json!(["2"]));
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("composite plan trailer");
        assert_eq!(page["eligible"], "3");
        assert_eq!(page["order_by"], serde_json::json!([semantic]));
    }
}

#[test]
fn descending_rows_cross_a_page_boundary_without_repeating_or_skipping() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..33)
            .map(|minor| (100, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    fixture.finish();
    let base = target("rows", "field=minor&page_size=20&order=desc");
    let first = stream(fixture.prepare(&base, None)).expect("first descending page");
    assert_eq!(
        row_records(&first)
            .iter()
            .map(|row| row["ordinal"].as_str().expect("ordinal"))
            .collect::<Vec<_>>(),
        (13..33)
            .rev()
            .map(|ordinal| ordinal.to_string())
            .collect::<Vec<_>>()
    );
    let cursor = first
        .iter()
        .find(|record| record["record"] == "page")
        .and_then(|record| record["next_cursor"].as_str())
        .expect("descending cursor");
    let second = stream(fixture.prepare(&format!("{base}&cursor={cursor}"), None))
        .expect("second descending page");
    assert_eq!(
        row_records(&second)
            .iter()
            .map(|row| row["ordinal"].as_str().expect("ordinal"))
            .collect::<Vec<_>>(),
        (0..13)
            .rev()
            .map(|ordinal| ordinal.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        second
            .iter()
            .find(|record| record["record"] == "page")
            .expect("page trailer")["next_cursor"],
        Value::Null
    );
}

#[test]
fn composite_statement_cursor_keeps_the_exact_global_order() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=queryid&by=derived.mean_exec_ms_per_call&page_size=1"
    );
    let mut cursor = None;
    let mut queryids = Vec::new();
    loop {
        let target = cursor
            .as_ref()
            .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
        let records = stream(fixture.prepare(&target, None)).expect("composite statement cursor");
        queryids.push(row_records(&records)[0]["values"][0].clone());
        cursor = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .and_then(|page| page["next_cursor"].as_str())
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        queryids,
        [
            serde_json::json!("2"),
            serde_json::json!("1"),
            serde_json::json!("3"),
        ]
    );
}

#[test]
fn snapshot_pages_with_tied_rates_have_no_duplicates_or_omissions() {
    let mut fixture = Fixture::new();
    let mut rows = Vec::new();
    for minor in 0..5 {
        rows.push((100, minor, 10));
        rows.push((200, minor, 11));
    }
    fixture.append_diskstats(&rows);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&by=reads&page_size=2"
    );
    let mut cursor = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let target = cursor
            .as_ref()
            .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
        let records = stream(fixture.prepare(&target, None)).expect("snapshot page");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("page trailer");
        let page_rows = row_records(&records);
        assert_eq!(page["eligible"], "5");
        assert_eq!(page["returned"], page_rows.len().to_string());
        assert_eq!(page["truncated"], page_rows.len() < 5);
        for row in page_rows {
            let ordinal = row["ordinal"].as_str().expect("ordinal").to_owned();
            assert!(!seen.contains(&ordinal), "a cursor must not repeat a row");
            seen.push(ordinal);
        }
        pages += 1;
        cursor = page["next_cursor"].as_str().map(ToOwned::to_owned);
        assert_eq!(page["has_more"], cursor.is_some());
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, ["1", "3", "5", "7", "9"]);
}

#[test]
fn active_snapshot_cursor_pins_the_original_wal_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (100, 1, 1), (100, 2, 1)]);
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&by=minor&page_size=2"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first active page");
    let cursor = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .and_then(|page| page["next_cursor"].as_str())
        .expect("active cursor")
        .to_owned();

    fixture.append_diskstats(&[(100, 3, 1), (100, 4, 1)]);
    let continued = stream(fixture.prepare(&format!("{base}&cursor={cursor}"), None))
        .expect("pinned active page");
    assert_eq!(row_records(&continued)[0]["values"], serde_json::json!([0]));
    let page = continued
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("continued page trailer");
    assert_eq!(page["eligible"], "3");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);

    let fresh = stream(fixture.prepare(&base, None)).expect("fresh active page");
    let page = fresh
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("fresh page trailer");
    assert_eq!(page["eligible"], "5");

    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    let (path, query) = format!("{base}&cursor={cursor}")
        .split_once('?')
        .map(|(path, query)| (path.to_owned(), query.to_owned()))
        .expect("finished cursor target");
    let route = crate::route::parse(&path, Some(&query)).expect("finished cursor route");
    assert!(matches!(
        crate::api::prepare(fixture.root(), SOURCES, route, None),
        Err(ApiError::BadCursor)
    ));
}

#[test]
fn exact_large_deltas_resets_and_missing_predecessors_survive_pages() {
    let mut fixture = Fixture::new();
    let base = 1_i64 << 53;
    fixture.append_diskstats(&[
        (100, 0, base),
        (200, 0, base + 1),
        (100, 1, base),
        (200, 1, base + 2),
        (100, 2, 20),
        (200, 2, 10),
        (200, 3, 7),
    ]);
    fixture.finish();

    let base_target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&by=reads&page_size=2"
    );
    let first = stream(fixture.prepare(&base_target, None)).expect("first exact page");
    assert_eq!(
        row_records(&first)
            .into_iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [
            serde_json::json!([1, 20_000.0]),
            serde_json::json!([0, 10_000.0]),
        ]
    );
    let cursor = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .and_then(|page| page["next_cursor"].as_str())
        .expect("second page cursor");
    let second = stream(fixture.prepare(&format!("{base_target}&cursor={cursor}"), None))
        .expect("second exact page");
    assert_eq!(
        row_records(&second)
            .into_iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [serde_json::json!([2, null]), serde_json::json!([3, null])]
    );
}

#[test]
fn statement_search_finds_a_match_outside_the_old_first_two_hundred() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(205);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&field=query&by=queryid&page_size=200"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first statement page");
    let rows = row_records(&first);
    assert_eq!(rows.len(), 200);
    assert!(rows.iter().all(|row| row["values"][0] != "0"));
    let page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("statement page trailer");
    assert_eq!(page["eligible"], "205");
    assert_eq!(page["has_more"], true);

    let searched = stream(fixture.prepare(&format!("{base}&search=OWNER*BLOCKER%3FNEEDLE"), None))
        .expect("server-side statement search");
    let rows = row_records(&searched);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "0");
    let page = searched
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("search page trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");

    assert_eq!(page["has_more"], false);
    assert_eq!(page["truncated"], false);
}

#[test]
fn structured_statement_search_filters_the_full_set_before_sort_and_page() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(205);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&field=query&by=queryid&page_size=200&search=query_id%3A0%20AND%20text%3A%22owner%20blocker-needle%20outside%20page%20one%22"
    );
    let records = stream(fixture.prepare(&target, None)).expect("structured statement search");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["values"],
        serde_json::json!(["0", "owner blocker-needle outside page one"])
    );
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("structured statement page trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);

    let or_target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&field=query&by=queryid&page_size=1&search=query_id%3A0%20OR%20query_id%3A1"
    );
    let or_records = stream(fixture.prepare(&or_target, None)).expect("structured statement OR");
    let or_page = or_records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("structured statement OR trailer");
    assert_eq!(or_page["eligible"], "2");
    assert_eq!(or_page["returned"], "1");
    assert_eq!(or_page["has_more"], true);
}

#[test]
fn structured_statement_search_preserves_bigint_text_across_the_api() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[
        (100, 9_007_199_254_740_993, 1, 1.0),
        (100, -9_007_199_254_740_993, 1, 1.0),
    ]);
    fixture.finish();

    for query_id in ["9007199254740993", "-9007199254740993"] {
        let target = format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&by=queryid&page_size=1&search=query_id%3A{query_id}"
        );
        let records = stream(fixture.prepare(&target, None)).expect("exact bigint search");
        let rows = row_records(&records);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["values"][0], query_id);
    }
}

#[test]
fn numeric_statement_page_returns_the_full_requested_page() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(205);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&field=query&field=calls&field=total_exec_time&by=total_exec_time&page_size=200&text=160"
    );
    let records = stream(fixture.prepare(&target, None)).expect("numeric statement page");
    assert_eq!(row_records(&records).len(), 200);
}

#[test]
fn postgres_summary_is_one_hour_series_for_all_surfaces() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[
        (100, 1, 10, 100.0),
        (100, 2, 20, 300.0),
        (200, 1, 14, 160.0),
        (200, 2, 20, 300.0),
    ]);
    fixture.append_plan_snapshots(&[
        (100, 1, 2, 20.0),
        (100, 2, 4, 60.0),
        (200, 1, 5, 50.0),
        (200, 2, 4, 60.0),
    ]);
    fixture.append_postgres_summary_relations();
    fixture.finish();

    let prepared = fixture.prepare(
        "/api/hour?from=0&to=3599999999&section=postgresql_summary",
        None,
    );
    let mut bytes = Vec::new();
    prepared
        .stream(
            &mut |record| {
                bytes.extend(record);
                true
            },
            &|| false,
        )
        .expect("PostgreSQL summary stream");
    let body = String::from_utf8(bytes).expect("UTF-8 PostgreSQL summary");
    assert!(body.contains(r#""logical_name":"postgresql_summary""#));
    assert!(body.contains(r#""name":"surface""#));
    for surface in 1..=5 {
        assert!(body.contains(&format!(r#""values":[{surface},"#)));
    }

    let records = body
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("PostgreSQL summary NDJSON"))
        .collect::<Vec<_>>();
    let columns = records
        .iter()
        .find(|record| record["record"] == "layout")
        .and_then(|record| record["layout"]["columns"].as_array())
        .expect("PostgreSQL summary columns");
    let column = |name: &str| {
        columns
            .iter()
            .position(|column| column["name"] == name)
            .expect("PostgreSQL summary fact")
    };
    let row = |surface: u64| {
        records
            .iter()
            .find(|record| {
                record["record"] == "row"
                    && record["timestamp"] == "200"
                    && record["values"][0] == surface
            })
            .expect("PostgreSQL summary surface row")
    };
    assert_eq!(row(1)["values"][column("active_pct")], 50.0);
    assert_eq!(row(1)["values"][column("mean_exec_ms")], 15.0);
    assert_eq!(row(3)["values"][column("rollback_pct")], 20.0);
    assert_eq!(row(4)["values"][column("seq_scan_pct")], 40.0);
    assert_eq!(row(5)["values"][column("scanned_pct")], 50.0);
}

#[test]
fn postgres_summary_uses_only_the_adjacent_physical_segment_as_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 100.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_diskstats(&[(150, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_statement_snapshots(&[(200, 1, 20, 300.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 3_000);
    fixture.append_statement_snapshots(&[(300, 1, 30, 500.0)]);
    fixture.finish();

    let records =
        stream(fixture.prepare("/api/hour?from=150&to=200&section=postgresql_summary", None))
            .expect("PostgreSQL summary with an adjacent non-PostgreSQL segment");
    let layout = records
        .iter()
        .find(|record| record["record"] == "layout")
        .expect("PostgreSQL summary layout");
    let mean = layout["layout"]["columns"]
        .as_array()
        .expect("PostgreSQL summary columns")
        .iter()
        .position(|column| column["name"] == "mean_exec_ms")
        .expect("mean execution column");
    let statement = records
        .iter()
        .find(|record| record["record"] == "row" && record["values"][0] == 1)
        .expect("statement summary row");
    assert_eq!(statement["values"][mean], Value::Null);

    let adjacent =
        stream(fixture.prepare("/api/hour?from=300&to=300&section=postgresql_summary", None))
            .expect("PostgreSQL summary with an adjacent PostgreSQL segment");
    let statement = adjacent
        .iter()
        .find(|record| record["record"] == "row" && record["values"][0] == 1)
        .expect("statement summary row with predecessor");
    assert_eq!(statement["values"][mean], 20.0);
}

#[test]
fn statement_page_composes_a_snapshot_split_across_segments() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 100.0), (100, 2, 10, 100.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_statement_snapshots(&[(200, 1, 20, 300.0)]);
    let current_segment = SEGMENT_ID + 2_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_statement_snapshots(&[(200, 2, 20, 200.0)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_stat_statements&field=queryid&field=calls&field=total_exec_time&by=derived.mean_exec_ms_per_call&page_size=1"
    );
    let first = stream(fixture.prepare(&target, None)).expect("split statement page");
    let rows = row_records(&first);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "1");
    assert_eq!(rows[0]["values"][1], 100_000.0);
    assert_eq!(rows[0]["values"][2], 2_000_000.0);
    let page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("split statement page trailer");
    assert_eq!(page["eligible"], "2");
    assert_eq!(page["from"], "100");
    assert_eq!(page["to"], "200");
    assert_eq!(page["has_more"], true);

    let cursor = page["next_cursor"].as_str().expect("split page cursor");
    let second = stream(fixture.prepare(&format!("{target}&cursor={cursor}"), None))
        .expect("second split statement page");
    let rows = row_records(&second);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "2");
    assert_eq!(rows[0]["values"][1], 100_000.0);
    assert_eq!(rows[0]["values"][2], 1_000_000.0);
}

#[test]
fn statement_page_composes_every_segment_at_both_selected_moments() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 10.0), (100, 2, 20, 20.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_statement_snapshots(&[(100, 3, 30, 30.0), (100, 4, 40, 40.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_statement_snapshots(&[(200, 1, 20, 50.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 3_000);
    fixture.append_statement_snapshots(&[(200, 2, 30, 80.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 4_000);
    fixture.append_statement_snapshots(&[(200, 3, 40, 120.0)]);
    let current_segment = SEGMENT_ID + 5_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_statement_snapshots(&[(200, 4, 50, 170.0)]);
    fixture.finish();

    let base = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_stat_statements&field=queryid&field=query&field=calls&field=total_exec_time&by=total_exec_time&page_size=2&search=BOUNDARY*"
    );
    let mut cursor = None;
    let mut rows = Vec::new();
    let mut pages = 0;
    loop {
        let target = cursor
            .as_ref()
            .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
        let records = stream(fixture.prepare(&target, None)).expect("composed statement page");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("composed statement page trailer");
        assert_eq!(page["eligible"], "4");
        assert_eq!(page["returned"], "2");
        assert_eq!(page["from"], "100");
        assert_eq!(page["to"], "200");
        rows.extend(row_records(&records).into_iter().cloned());
        pages += 1;
        cursor = page["next_cursor"].as_str().map(ToOwned::to_owned);
        assert_eq!(page["has_more"], cursor.is_some());
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(pages, 2);
    assert_eq!(
        rows.iter()
            .map(|row| (
                row["segment_id"]
                    .as_str()
                    .expect("physical segment")
                    .to_owned(),
                row["values"].clone(),
            ))
            .collect::<Vec<_>>(),
        [
            (
                (SEGMENT_ID + 5_000).to_string(),
                serde_json::json!(["4", "boundary statement 4", 100_000.0, 1_300_000.0]),
            ),
            (
                (SEGMENT_ID + 4_000).to_string(),
                serde_json::json!(["3", "boundary statement 3", 100_000.0, 900_000.0]),
            ),
            (
                (SEGMENT_ID + 3_000).to_string(),
                serde_json::json!(["2", "boundary statement 2", 100_000.0, 600_000.0]),
            ),
            (
                (SEGMENT_ID + 2_000).to_string(),
                serde_json::json!(["1", "boundary statement 1", 100_000.0, 400_000.0]),
            ),
        ]
    );
}

#[test]
fn statement_and_plan_quantities_filter_hidden_metrics_before_paging() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.append_ranked_plans();
    fixture.append_postgres_block_size(8_192);
    fixture.finish();

    for section in ["pg_stat_statements", "pg_store_plans"] {
        let base = format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section={section}&field=queryid&by=queryid"
        );
        let records = stream(fixture.prepare(
            &format!("{base}&page_size=1&search=exec_time_rate%3E500000ms%2Fs"),
            None,
        ))
        .expect("hidden execution-load search");
        assert_eq!(row_records(&records)[0]["values"][0], "1", "{section}");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("quantity page trailer");
        assert_eq!(page["eligible"], "1", "{section}");
        assert_eq!(page["has_more"], false, "{section}");

        let strict = stream(fixture.prepare(
            &format!("{base}&page_size=10&search=mean_exec%3E10ms"),
            None,
        ))
        .expect("strict mean boundary");
        assert_eq!(row_records(&strict)[0]["values"][0], "2", "{section}");

        let hit = stream(fixture.prepare(
            &format!("{base}&page_size=10&search=buffer_hit%3E80%25"),
            None,
        ))
        .expect("strict hit boundary");
        assert_eq!(row_records(&hit)[0]["values"][0], "2", "{section}");

        let block_rate = stream(fixture.prepare(
            &format!(
                "{base}&page_size=10&search=query_id%3A1%20AND%20shared_buffer_read_rate%3E1638399999B%2Fs"
            ),
            None,
        ))
        .expect("exact buffer byte-rate boundary");
        assert_eq!(row_records(&block_rate)[0]["values"][0], "1", "{section}");

        let block_rate_boundary = stream(fixture.prepare(
            &format!(
                "{base}&page_size=10&search=query_id%3A1%20AND%20shared_buffer_read_rate%3E1638400000B%2Fs"
            ),
            None,
        ))
        .expect("strict buffer byte-rate equality");
        assert!(row_records(&block_rate_boundary).is_empty(), "{section}");

        let per_call = stream(fixture.prepare(
            &format!("{base}&page_size=10&search=query_id%3A1%20AND%20buffer_per_call%3E81919B"),
            None,
        ))
        .expect("exact buffer bytes per call");
        assert_eq!(row_records(&per_call)[0]["values"][0], "1", "{section}");
    }

    let grouped = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=queryid&by=queryid&page_size=10&search=query_id%3A3%20OR%20%28exec_time_rate%3E500000ms%2Fs%20AND%20rows_per_call%3E5%29"
        ),
        None,
    ))
    .expect("mixed grouped statement search");
    assert_eq!(
        row_records(&grouped)
            .iter()
            .map(|row| row["values"][0].as_str().expect("query id"))
            .collect::<Vec<_>>(),
        ["3", "1"]
    );
}

#[test]
fn quantitative_search_keeps_layout_absence_null() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.append_ranked_plans();
    fixture.finish();

    for (section, search) in [
        ("pg_stat_statements", "temp_read_time_rate%3C1ms%2Fs"),
        ("pg_store_plans", "planning_time_rate%3C1ms%2Fs"),
    ] {
        let records = stream(fixture.prepare(
            &format!(
                "/api/segments/{SEGMENT_ID}/snapshot?at=200&section={section}&field=queryid&by=queryid&page_size=10&search={search}"
            ),
            None,
        ))
        .expect("layout-unavailable quantity search");
        assert!(row_records(&records).is_empty(), "{section}");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("unavailable page trailer");
        assert_eq!(page["eligible"], "0", "{section}");
    }
}

#[test]
fn postgres_rates_use_adjacent_samples_without_optional_proof_metadata() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.append_ranked_plans();
    fixture.finish();

    for section in ["pg_stat_statements", "pg_store_plans"] {
        let records = stream(fixture.prepare(
            &format!(
                "/api/segments/{SEGMENT_ID}/snapshot?at=200&section={section}&field=queryid&by=calls&page_size=10&search=call_rate%3E1%2Fs"
            ),
            None,
        ))
        .expect("data-first PostgreSQL rates");
        assert_eq!(row_records(&records)[0]["values"][0], "1", "{section}");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("data-first page trailer");
        assert_eq!(page["eligible"], "3", "{section}");
        assert_eq!(page["order_by"], serde_json::json!(["calls"]), "{section}");
    }

    let mut exact_fixture = Fixture::new();
    exact_fixture.append_plan_snapshots(&[(100, 1, 9_007_199_254_740_993, 1.0), (100, 3, 1, 1.0)]);
    exact_fixture.finish();
    for (search, expected) in [("calls%3E9007199254740992", "1"), ("calls%3C2", "3")] {
        let records = stream(exact_fixture.prepare(
            &format!(
                "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_store_plans&field=queryid&by=queryid&page_size=1&search={search}"
            ),
            None,
        ))
        .expect("exact hidden Calls search");
        assert_eq!(row_records(&records)[0]["values"][0], expected);
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("exact Calls page trailer");
        assert_eq!(page["eligible"], "1");
        assert_eq!(page["has_more"], false);
    }
}

#[test]
fn vadv_plan_quantities_include_planning_and_slow_calls() {
    let mut fixture = Fixture::new();
    fixture.append_vadv_plan_quantities();
    fixture.finish();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_store_plans&field=queryid&by=queryid&page_size=10"
    );
    for search in [
        "planning_time_rate%3E249999ms%2Fs",
        "planning_share%3E24.9%25",
        "slow_call_rate%3E39999%2Fs",
    ] {
        let records = stream(fixture.prepare(&format!("{base}&search={search}"), None))
            .expect("vadv quantity search");
        assert_eq!(row_records(&records)[0]["values"][0], "1", "{search}");
    }
    let strict = stream(fixture.prepare(&format!("{base}&search=planning_share%3E25%25"), None))
        .expect("strict planning share");
    assert!(row_records(&strict).is_empty());
}

#[test]
fn related_statement_search_uses_the_exact_cursor_across_segments() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 42, 10, 10.0), (200, 42, 20, 20.0)]);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_statement_snapshots(&[(200, 99, 30, 30.0), (300, 98, 40, 40.0)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_stat_statements&field=queryid&field=dbid&field=userid&field=datname&field=usename&field=query&by=queryid&page_size=32&search=database%3Aoperators%20AND%20role%3Areporter%20AND%20query_id%3A42"
    );
    let records = stream(fixture.prepare(&target, None)).expect("related statement search");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(rows[0]["timestamp"], "200");
    assert_eq!(
        rows[0]["values"],
        serde_json::json!([
            "42",
            73,
            72,
            "operators",
            "reporter",
            "boundary statement 42"
        ])
    );
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("related statement page trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
    assert_eq!(page["from"], "100");
    assert_eq!(page["to"], "200");

    let first_target = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A42&first_match=1"
    );
    let first_records =
        stream(fixture.prepare(&first_target, None)).expect("first Statement text at cursor");
    let first_rows = row_records(&first_records);
    assert_eq!(first_rows.len(), 1);
    assert_eq!(first_rows[0]["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(first_rows[0]["timestamp"], "200");
    assert_eq!(
        first_rows[0]["values"],
        serde_json::json!(["boundary statement 42"])
    );
}

#[test]
fn statement_text_first_match_returns_the_first_nonempty_record() {
    let mut fixture = Fixture::new();
    let exact = "  SELECT *\n  FROM work_queue\n";
    let mut texts = vec![None, Some(exact)];
    texts.extend(std::iter::repeat_n(Some("later collision"), 128));
    fixture.append_statement_text_matches(200, -42, &texts);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A-42&first_match=1"
    );
    let records = stream(fixture.prepare(&target, None)).expect("first Statement text");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([exact]));
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("first-match trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
    assert_eq!(page["page_size"], 1);
    assert_eq!(page["order_by"], serde_json::json!([]));
}

#[test]
fn statement_text_first_match_rejects_a_general_search_expression() {
    let mut fixture = Fixture::new();
    fixture.append_statement_text_matches(200, 42, &[Some("select 42")]);
    fixture.finish();
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=query&page_size=1&search=database%3Aoperators%20AND%20query_id%3A42&first_match=1"
    );
    let (path, query) = target.split_once('?').expect("snapshot target");
    let route = crate::route::parse(path, Some(query)).expect("strict route shape");
    assert!(matches!(
        crate::api::prepare(fixture.root(), SOURCES, route, None),
        Err(ApiError::BadFilter(name)) if name == "first_match"
    ));
}

#[test]
fn plan_page_keeps_zero_and_rejects_a_cross_segment_counter_decrease() {
    let mut fixture = Fixture::new();
    fixture.append_plan_snapshots(&[(100, 1, 4, 40.0), (100, 2, 10, 100.0)]);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_plan_snapshots(&[(200, 1, 4, 40.0), (200, 2, 5, 50.0)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_store_plans&field=queryid&field=calls&field=total_time&by=queryid&page_size=200"
    );
    let records = stream(fixture.prepare(&target, None)).expect("cross-segment plan page");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 2);
    let rows = rows
        .into_iter()
        .map(|row| (row["values"][0].as_str().expect("queryid"), row))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(rows["1"]["values"][1], "4");
    assert_eq!(rows["1"]["values"][2], 0.0);
    assert_eq!(rows["1"]["values"][3], 0.0);
    assert_eq!(rows["2"]["values"][1], "5");
    assert_eq!(rows["2"]["values"][2], Value::Null);
    assert_eq!(rows["2"]["values"][3], Value::Null);
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("cross-segment plan page trailer");
    assert_eq!(page["from"], "100");
    assert_eq!(page["to"], "200");
}

#[test]
fn plan_search_finds_a_match_outside_the_old_first_two_hundred() {
    let mut fixture = Fixture::new();
    fixture.append_plan_universe(205);
    fixture.finish();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_store_plans&field=queryid&field=plan&by=queryid&page_size=200"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first plan page");
    assert_eq!(row_records(&first).len(), 200);
    assert!(
        row_records(&first)
            .iter()
            .all(|row| row["values"][0] != "0")
    );

    let searched = stream(fixture.prepare(&format!("{base}&search=PLAN%3FNEEDLE"), None))
        .expect("server-side plan search");
    let rows = row_records(&searched);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "0");
    let page = searched
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("plan search trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
}

#[test]
fn process_user_search_filters_the_full_set_and_keeps_real_and_effective_names_distinct() {
    let mut fixture = Fixture::new();
    let processes = (0..205)
        .map(|pid| {
            if pid == 0 {
                (pid, 26, 27, 0, 0)
            } else if pid == 1 {
                (pid, 9_999, 9_999, 0, 0)
            } else {
                (pid, 1_000, 1_000, 0, 0)
            }
        })
        .collect::<Vec<_>>();
    fixture.append_user_processes(
        100,
        &processes,
        &[(26, "postgres"), (27, "postgres-worker"), (1_000, "app")],
    );
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_process&field=pid&field=user&field=effective_user&field=uid&field=euid&by=pid&page_size=1"
    );
    let records = stream(fixture.prepare(&format!("{base}&search=username%3Apostgres"), None))
        .expect("real user search");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["values"],
        serde_json::json!([0, "postgres", "postgres-worker", 26, 27])
    );
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("process page trailer");
    assert_eq!(page["eligible"], "1");

    let effective =
        stream(fixture.prepare(&format!("{base}&search=euser%3Apostgres-worker"), None))
            .expect("effective user search");
    assert_eq!(row_records(&effective)[0]["values"][0], 0);

    let unresolved = stream(fixture.prepare(&format!("{base}&search=user_id%3A9999"), None))
        .expect("numeric unresolved user search");
    assert_eq!(
        row_records(&unresolved)[0]["values"],
        serde_json::json!([1, null, null, 9_999, 9_999])
    );
}

#[test]
fn process_quantities_use_same_starttime_predecessors_and_exact_counters() {
    let mut fixture = Fixture::new();
    fixture.append_quantitative_processes();
    fixture.finish();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=2000000&section=os_process&field=pid&field=read_bytes&field=rmem_kb&by=pid&page_size=10"
    );

    let natural = stream(fixture.prepare(
        &format!("{base}&search=cpu_cores%3E0.1%20AND%20rss%3E2MiB"),
        None,
    ))
    .expect("natural process quantity search");
    assert_eq!(
        row_records(&natural)
            .iter()
            .map(|row| row["values"][0].as_i64().expect("pid"))
            .collect::<Vec<_>>(),
        [1]
    );

    let bytes = stream(fixture.prepare(
        &format!("{base}&search=disk_read_rate%3E1048575B%2Fs"),
        None,
    ))
    .expect("bigint-safe process byte rate");
    assert_eq!(
        row_records(&bytes)
            .iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [serde_json::json!([1, 1_048_576.0, "3072"])]
    );

    for search in [
        "logical_read_rate%3E1048575B%2Fs",
        "read_syscall_rate%3E19%2Fs",
        "run_delay%3E19ms%2Fs",
        "block_io_delay%3E199ms%2Fs",
    ] {
        let records = stream(fixture.prepare(&format!("{base}&search={search}"), None))
            .expect("derived process rate");
        assert_eq!(row_records(&records)[0]["values"][0], 1, "{search}");
    }

    let exact_zero = stream(fixture.prepare(
        &format!("{base}&search=pid%3A6%20AND%20cpu_cores%3C0.1"),
        None,
    ))
    .expect("stable zero CPU rate");
    assert_eq!(row_records(&exact_zero)[0]["values"][0], 6);

    let optional_null = stream(fixture.prepare(
        &format!("{base}&search=pid%3A6%20AND%20read_syscall_rate%3C1%2Fs"),
        None,
    ))
    .expect("missing optional process I/O");
    assert!(row_records(&optional_null).is_empty());

    let unavailable = stream(fixture.prepare(&format!("{base}&search=cpu_cores%3C1000"), None))
        .expect("process predecessor exclusions");
    assert_eq!(
        row_records(&unavailable)
            .iter()
            .map(|row| row["values"][0].as_i64().expect("pid"))
            .collect::<Vec<_>>(),
        [6, 2, 1],
        "PID reuse, rollback, and a missing predecessor stay null"
    );

    let all = stream(fixture.prepare(&base, None)).expect("unfiltered process quantities");
    let by_pid = row_records(&all)
        .into_iter()
        .map(|row| (row["values"][0].as_i64().expect("pid"), row))
        .collect::<BTreeMap<_, _>>();
    for pid in [3, 4, 5] {
        assert_eq!(by_pid[&pid]["values"][1], Value::Null, "pid {pid}");
    }
    assert_eq!(by_pid[&2]["values"][2], "2048");
}

#[test]
fn process_user_join_uses_the_mapping_from_each_historical_segment() {
    let mut fixture = Fixture::new();
    fixture.append_user_processes(100, &[(10, 26, 26, 0, 0)], &[(26, "old-name")]);
    let second_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(second_segment);
    fixture.append_user_processes(200, &[(10, 26, 26, 0, 0)], &[(26, "new-name")]);
    fixture.finish();

    for (segment, at, expected) in [
        (SEGMENT_ID, 100, "old-name"),
        (second_segment, 200, "new-name"),
    ] {
        let target = format!(
            "/api/segments/{segment}/snapshot?at={at}&section=os_process&field=pid&field=user&field=uid&by=pid&page_size=10"
        );
        let records = stream(fixture.prepare(&target, None)).expect("historical process snapshot");
        assert_eq!(
            row_records(&records)[0]["values"],
            serde_json::json!([10, expected, 26])
        );
    }
}

#[test]
fn snapshot_cursor_rejects_every_bound_query_shape_mismatch() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(5);
    fixture.finish();
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    let shape = "at=100&section=pg_stat_statements&field=queryid&field=query&by=queryid&by=userid&page_size=1&search=text%3Afixture%20AND%20text%3Astatement&text=80&where.dbid=73&where.userid=72&type_id=1002002";
    let first = stream(fixture.prepare(&format!("{path}?{shape}"), None)).expect("bound page");
    let cursor = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .and_then(|page| page["next_cursor"].as_str())
        .expect("bound cursor");

    let mismatches = [
        (SEGMENT_ID + 1, shape.to_owned()),
        (SEGMENT_ID, shape.replacen("at=100", "at=99", 1)),
        (
            SEGMENT_ID,
            shape.replacen("section=pg_stat_statements", "section=os_diskstats", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen("field=queryid&field=query", "field=query&field=queryid", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen("by=queryid&by=userid", "by=userid&by=queryid", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen(
                "search=text%3Afixture%20AND%20text%3Astatement",
                "search=text%3Astatement%20AND%20text%3Afixture",
                1,
            ),
        ),
        (SEGMENT_ID, shape.replacen("text=80", "text=81", 1)),
        (
            SEGMENT_ID,
            shape.replacen("where.dbid=73", "where.dbid=74", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen(
                "where.dbid=73&where.userid=72",
                "where.userid=72&where.dbid=73",
                1,
            ),
        ),
        (
            SEGMENT_ID,
            shape.replacen("type_id=1002002", "type_id=1002001", 1),
        ),
    ];
    for (segment_id, query) in mismatches {
        let path = format!("/api/segments/{segment_id}/snapshot");
        let query = format!("{query}&cursor={cursor}");
        let route = crate::route::parse(&path, Some(&query)).expect("mismatch route");
        let result = crate::api::prepare(fixture.root(), SOURCES, route, None);
        assert!(matches!(result, Err(ApiError::BadCursor)), "{query}");
    }

    let compatible_size = shape.replacen("page_size=1", "page_size=4", 1);
    let query = format!("{compatible_size}&cursor={cursor}");
    let route = crate::route::parse(&path, Some(&query)).expect("compatible route");
    assert!(crate::api::prepare(fixture.root(), SOURCES, route, None).is_ok());
}

#[test]
fn snapshot_search_uses_the_fixed_section_allowlist_outside_the_projection() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(1);
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish();
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    for query in [
        "at=100&section=pg_stat_statements&field=calls&search=needle",
        "at=100&section=pg_stat_statements&search=needle",
    ] {
        let route = crate::route::parse(&path, Some(query)).expect("search route");
        assert!(crate::api::prepare(fixture.root(), SOURCES, route, None).is_ok());
    }
    let route = crate::route::parse(
        &path,
        Some("at=100&section=os_diskstats&field=device&search=needle"),
    )
    .expect("unsupported search route");
    assert!(matches!(
        crate::api::prepare(fixture.root(), SOURCES, route, None),
        Err(ApiError::BadFilter(parameter)) if parameter == "search"
    ));
}

#[test]
fn snapshot_resolves_text_only_after_page_rows_are_selected() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_diskstats_with_unreadable_loser();
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&field=device&by=minor&page_size=1"
    );
    let records = stream(fixture.prepare(&target, None)).expect("top row snapshot");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([2, "nvme0n1"]));
}

#[test]
fn a_snapshot_projects_one_exact_physical_source_row() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[
        (100, 0, 1),
        (100, 1, 9),
        (100, 2, 5),
        (200, 0, 2),
        (200, 1, 30),
        (200, 2, 11),
    ]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&type_id=1108001&row_ordinal=3"
    );
    let records = stream(fixture.prepare(&target, None)).expect("exact projected snapshot");
    let layout = records
        .iter()
        .find(|record| record["record"] == "layout")
        .expect("layout");
    assert_eq!(layout["layout"]["type_id"], "1108001");
    assert_eq!(
        layout["layout"]["columns"],
        serde_json::json!([
            {"name": "minor", "type": "i32", "class": "label", "unit": "none", "nullable": false, "available": true},
            {"name": "reads", "type": "i64", "class": "cumulative", "unit": "count", "nullable": false, "available": true}
        ])
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ordinal"], "3");
    assert_eq!(rows[0]["timestamp"], "200");
    assert_eq!(rows[0]["values"], serde_json::json!([1, 210_000.0]));
}

#[test]
fn an_exact_source_pointer_must_name_a_captured_row_at_its_timestamp() {
    let mut active = Fixture::new();
    active.append_diskstats(&[(200, 0, 2)]);
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0"
    );
    let records = stream(active.prepare(&target, None)).expect("captured active row");
    assert_eq!(row_records(&records)[0]["values"], serde_json::json!([0]));

    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    let route = crate::route::parse(
        &path,
        Some("at=199&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0"),
    )
    .expect("exact route");
    assert!(matches!(
        crate::api::prepare(active.root(), SOURCES, route, None),
        Err(ApiError::BadCursor)
    ));

    active.finish();
    for query in [
        "at=199&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0",
        "at=200&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=1",
    ] {
        let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
        let route = crate::route::parse(&path, Some(query)).expect("exact route");
        assert!(matches!(
            crate::api::prepare(active.root(), SOURCES, route, None),
            Err(ApiError::BadCursor)
        ));
    }
}

#[test]
fn a_snapshot_keeps_only_the_rows_a_filter_names() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (100, 1, 9), (200, 0, 2), (200, 1, 30)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&where.minor=1"
    );
    let records = stream(fixture.prepare(&target, None)).expect("filtered snapshot");
    let minors = records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|record| record["values"][0].clone())
        .collect::<Vec<_>>();
    assert_eq!(minors, [serde_json::json!(1)]);
}

#[test]
fn a_cgroup_snapshot_applies_the_exact_path_and_scope_filters() {
    const ROWS: usize = 50_000;
    let mut fixture = Fixture::new();
    fixture.append_large_cgroup_cpu(ROWS, ROWS / 2);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_cgroup_cpu&field=cgroup_path&field=scope&where.cgroup_path=%2Fcollector&where.scope=3"
    );
    let records = stream(fixture.prepare(&target, None)).expect("filtered cgroup snapshot");
    let rows = row_records(&records);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!(["/collector", 3]));
}

#[test]
fn a_snapshot_rejects_a_dangling_exact_filter_id() {
    let mut fixture = Fixture::new();
    let wanted = kronika_format::StrId::of(b"sda")
        .expect("fixture filter id is nonzero")
        .get();
    let mut buffers = SectionBuffers::new();
    buffers
        .push(diskstats_with_device(100, 0, 1, StrId(wanted)))
        .expect("dangling filter row fits");
    fixture.append(buffers);

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&where.device=sda"
    );
    let error = fixture
        .prepare(&target, None)
        .stream(&mut |_record| true, &|| false)
        .expect_err("dangling requested filter id must fail");

    assert!(matches!(error, ApiError::Unreadable(_)));
    assert!(
        error
            .to_string()
            .contains(&format!("unresolved dictionary id {wanted}"))
    );
}

#[test]
fn table_snapshot_pages_use_each_database_moments_and_elapsed_time() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(
        &[
            (10_000_000, 1, 77, 10),
            (30_000_000, 1, 77, 30),
            (20_000_000, 2, 77, 5),
            (25_000_000, 2, 77, 15),
        ],
        &[],
        &[],
    );
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_tables&field=datid&field=relid&field=seq_scan&by=seq_scan&page_size=1"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first table page");
    assert_eq!(
        row_records(&first)[0]["values"],
        serde_json::json!([2, 77, 2.0])
    );
    let first_page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("first table page trailer");
    assert_eq!(first_page["eligible"], "2");
    assert_eq!(first_page["from"], "10000000");
    assert_eq!(first_page["to"], "30000000");
    let cursor = first_page["next_cursor"].as_str().expect("table cursor");

    let second = stream(fixture.prepare(&format!("{base}&cursor={cursor}"), None))
        .expect("second table page");
    assert_eq!(
        row_records(&second)[0]["values"],
        serde_json::json!([1, 77, 1.0])
    );
    let second_page = second
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("second table page trailer");
    assert_eq!(second_page["eligible"], "2");
    assert_eq!(second_page["has_more"], false);
}

#[test]
fn relation_table_buffer_rates_distinguish_values_zero_and_missing_predecessors() {
    let mut fixture = Fixture::new();
    fixture.append_buffered_table_snapshots(&[
        (10_000_000, 1, 77, [100, 900, 50, 450, 10, 90, 5, 45]),
        (20_000_000, 1, 77, [110, 990, 55, 495, 11, 99, 6, 54]),
        (10_000_000, 2, 78, [8, 16, 4, 12, 2, 6, 1, 3]),
        (20_000_000, 2, 78, [8, 16, 4, 12, 2, 6, 1, 3]),
        (20_000_000, 3, 79, [10, 90, 5, 45, 1, 9, 1, 9]),
    ]);
    fixture.finish();

    let fields = [
        "heap_blks_read",
        "heap_blks_hit",
        "idx_blks_read",
        "idx_blks_hit",
        "toast_blks_read",
        "toast_blks_hit",
        "tidx_blks_read",
        "tidx_blks_hit",
        "heap_buffer_hit_pct",
        "index_buffer_hit_pct",
        "toast_buffer_hit_pct",
        "tidx_buffer_hit_pct",
        "buffer_hit_pct",
    ]
    .map(|field| format!("field={field}"))
    .join("&");
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&group=object&{fields}"
    );
    let records = stream(fixture.prepare(&target, None)).expect("buffer relation snapshot");
    let rows = relation_records(&records)
        .into_iter()
        .map(|row| (row["key"]["datid"].as_str().unwrap().to_owned(), row))
        .collect::<BTreeMap<_, _>>();

    let assert_rate = |field: &str, expected: f64| {
        let actual = rows["1"]["values"][field]
            .as_f64()
            .expect("numeric buffer rate");
        assert!((actual - expected).abs() < 1e-12, "{field}: {actual}");
    };
    for (field, expected) in [
        ("heap_blks_read", 1.0),
        ("heap_blks_hit", 9.0),
        ("idx_blks_read", 0.5),
        ("idx_blks_hit", 4.5),
        ("toast_blks_read", 0.1),
        ("toast_blks_hit", 0.9),
        ("tidx_blks_read", 0.1),
        ("tidx_blks_hit", 0.9),
        ("heap_buffer_hit_pct", 90.0),
        ("index_buffer_hit_pct", 90.0),
        ("toast_buffer_hit_pct", 90.0),
        ("tidx_buffer_hit_pct", 90.0),
        ("buffer_hit_pct", 90.0),
    ] {
        assert_rate(field, expected);
    }

    for field in fields
        .split('&')
        .map(|field| field.trim_start_matches("field="))
    {
        if !field.ends_with("_pct") {
            assert_eq!(rows["2"]["values"][field], 0.0, "true zero {field}");
            assert_eq!(
                rows["3"]["values"][field],
                Value::Null,
                "missing predecessor {field}"
            );
        }
    }
    for field in [
        "heap_buffer_hit_pct",
        "index_buffer_hit_pct",
        "toast_buffer_hit_pct",
        "tidx_buffer_hit_pct",
        "buffer_hit_pct",
    ] {
        assert_eq!(rows["2"]["values"][field], Value::Null);
        assert_eq!(rows["3"]["values"][field], Value::Null);
    }

    assert_exact_buffer_rows(&fixture);
}

fn assert_exact_buffer_rows(fixture: &Fixture) {
    let exact = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&field=datid&field=heap_blks_read&field=heap_blks_hit&type_id=1013005&row_ordinal=1"
    );
    let exact = stream(fixture.prepare(&exact, None)).expect("exact partitioned buffer row");
    assert_eq!(
        row_records(&exact)[0]["values"],
        serde_json::json!([1, 1.0, 9.0]),
        "the exact row uses its own database predecessor"
    );

    let exact_zero = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&field=datid&field=heap_blks_read&type_id=1013005&row_ordinal=3"
    );
    let exact_zero = stream(fixture.prepare(&exact_zero, None)).expect("exact zero buffer row");
    assert_eq!(
        row_records(&exact_zero)[0]["values"],
        serde_json::json!([2, 0.0])
    );

    let exact_missing = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&field=datid&field=heap_blks_read&type_id=1013005&row_ordinal=4"
    );
    let exact_missing =
        stream(fixture.prepare(&exact_missing, None)).expect("exact missing predecessor row");
    assert_eq!(
        row_records(&exact_missing)[0]["values"],
        serde_json::json!([3, null])
    );

    let version_absent = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&group=object&field=n_tup_newpage_upd&where.datid=1"
    );
    let version_absent =
        stream(fixture.prepare(&version_absent, None)).expect("version-absent relation field");
    assert_eq!(
        relation_records(&version_absent)[0]["values"]["n_tup_newpage_upd"],
        Value::Null,
        "a field absent from the physical layout is not a numeric zero"
    );
}

#[test]
fn relation_predecessor_skips_sectionless_segments_and_overlapping_bounds() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(
        100_000_000,
        1,
        77,
        10,
        "fixture_db",
        "public",
        "orders",
    )]);
    fixture.append_diskstats(&[(400_000_000, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_diskstats(&[(200_000_000, 0, 2)]);
    let current_segment = SEGMENT_ID + 2_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_named_table_snapshots(&[(
        300_000_000,
        1,
        77,
        30,
        "fixture_db",
        "public",
        "orders",
    )]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=300000000&section=pg_stat_user_tables&group=object&field=seq_scan"
    );
    let records = stream(fixture.prepare(&target, None)).expect("cross-segment relation snapshot");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["seq_scan"], 0.1);
    assert_eq!(rows[0]["sample_from"], "100000000");
    assert_eq!(rows[0]["sample_to"], "300000000");
}

#[test]
fn relation_predecessor_follows_a_fallback_sample_into_the_second_segment() {
    let mut fixture = Fixture::new();
    fixture.append_buffered_table_snapshots(&[(
        10_000_000,
        1,
        77,
        [100, 900, 50, 450, 10, 90, 5, 45],
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_buffered_table_snapshots(&[(
        20_000_000,
        1,
        77,
        [110, 990, 55, 495, 11, 99, 6, 54],
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_diskstats(&[(25_000_000, 0, 1)]);
    fixture.append_buffered_table_snapshots(&[(
        35_000_000,
        1,
        77,
        [120, 1_080, 60, 540, 12, 108, 7, 63],
    )]);
    let current_segment = SEGMENT_ID + 3_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_buffered_table_snapshots(&[(
        40_000_000,
        1,
        77,
        [120, 1_080, 60, 540, 12, 108, 7, 63],
    )]);
    fixture.finish();

    let fields = [
        "heap_blks_read",
        "heap_blks_hit",
        "idx_blks_read",
        "idx_blks_hit",
        "toast_blks_read",
        "toast_blks_hit",
        "tidx_blks_read",
        "tidx_blks_hit",
        "buffer_hit_pct",
    ]
    .map(|field| format!("field={field}"))
    .join("&");
    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=30000000&section=pg_stat_user_tables&group=object&{fields}"
    );
    let records = stream(fixture.prepare(&target, None)).expect("fallback relation snapshot");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sample_from"], "10000000");
    assert_eq!(rows[0]["sample_to"], "20000000");
    for (field, expected) in [
        ("heap_blks_read", 1.0),
        ("heap_blks_hit", 9.0),
        ("idx_blks_read", 0.5),
        ("idx_blks_hit", 4.5),
        ("toast_blks_read", 0.1),
        ("toast_blks_hit", 0.9),
        ("tidx_blks_read", 0.1),
        ("tidx_blks_hit", 0.9),
        ("buffer_hit_pct", 90.0),
    ] {
        let actual = rows[0]["values"][field]
            .as_f64()
            .expect("numeric buffer rate");
        assert!((actual - expected).abs() < 1e-12, "{field}: {actual}");
    }
}

#[test]
fn relation_pages_compose_split_current_and_predecessor_moments() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100_000_000, 1, 11, 10, "db", "public", "first")]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_named_table_snapshots(&[(100_000_000, 1, 12, 20, "db", "public", "second")]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_named_table_snapshots(&[(200_000_000, 1, 11, 30, "db", "public", "first")]);
    let current_segment = SEGMENT_ID + 3_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_named_table_snapshots(&[(200_000_000, 1, 12, 60, "db", "public", "second")]);
    fixture.finish();

    let base = format!(
        "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_tables&group=object&field=seq_scan&by=seq_scan&direction=desc&page_size=1&where.datid=1"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first split relation page");
    let first_row = &relation_records(&first)[0];
    assert_eq!(first_row["key"]["relid"], "12");
    assert_eq!(first_row["values"]["seq_scan"], 0.4);
    assert_eq!(
        first_row["source"]["segment_id"],
        current_segment.to_string()
    );
    let first_page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("first split relation trailer");
    assert_eq!(first_page["eligible"], "2");
    assert_eq!(first_page["from"], "100000000");
    assert_eq!(first_page["to"], "200000000");
    let cursor = first_page["next_cursor"]
        .as_str()
        .expect("split relation cursor");

    let second = stream(fixture.prepare(&format!("{base}&cursor={cursor}"), None))
        .expect("second split relation page");
    let second_row = &relation_records(&second)[0];
    assert_eq!(second_row["key"]["relid"], "11");
    assert_eq!(second_row["values"]["seq_scan"], 0.2);
    assert_eq!(
        second_row["source"]["segment_id"],
        (SEGMENT_ID + 2_000).to_string()
    );
    let second_page = second
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("second split relation trailer");
    assert_eq!(second_page["eligible"], "2");
    assert_eq!(second_page["has_more"], false);
}

fn four_segment_relation_fixture() -> (Fixture, i64) {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100_000_000, 1, 11, 10, "first", "public", "orders")]);
    fixture.append_named_index_snapshots(&[(
        100_000_000,
        1,
        101,
        20,
        "first",
        "public",
        "orders",
        "orders_pkey",
        "CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)",
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_named_table_snapshots(&[(120_000_000, 2, 21, 5, "second", "public", "events")]);
    fixture.append_named_index_snapshots(&[(
        120_000_000,
        2,
        201,
        7,
        "second",
        "public",
        "events",
        "events_pkey",
        "CREATE UNIQUE INDEX events_pkey ON public.events USING btree (id)",
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_named_table_snapshots(&[(180_000_000, 2, 21, 17, "second", "public", "events")]);
    fixture.append_named_index_snapshots(&[(
        180_000_000,
        2,
        201,
        19,
        "second",
        "public",
        "events",
        "events_pkey",
        "CREATE UNIQUE INDEX events_pkey ON public.events USING btree (id)",
    )]);
    let current_segment = SEGMENT_ID + 3_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_named_table_snapshots(&[(200_000_000, 1, 11, 30, "first", "public", "orders")]);
    fixture.append_named_index_snapshots(&[(
        200_000_000,
        1,
        101,
        50,
        "first",
        "public",
        "orders",
        "orders_pkey",
        "CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)",
    )]);
    fixture.finish();
    (fixture, current_segment)
}

#[test]
fn relation_object_snapshots_keep_each_database_predecessor_across_segments() {
    let (fixture, current_segment) = four_segment_relation_fixture();

    let tables = stream(fixture.prepare(
        &format!(
            "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_tables&group=object&field=seq_scan"
        ),
        None,
    ))
    .expect("four-segment table snapshot");
    let table_rates = relation_records(&tables)
        .into_iter()
        .map(|row| {
            (
                row["key"]["datid"].as_str().unwrap().to_owned(),
                row["values"]["seq_scan"].clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(table_rates["1"], serde_json::json!(0.2));
    assert_eq!(table_rates["2"], serde_json::json!(0.2));

    let first_database = stream(fixture.prepare(
        &format!(
            "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_tables&group=object&field=seq_scan&where.datid=1"
        ),
        None,
    ))
    .expect("filtered four-segment table snapshot");
    let rows = relation_records(&first_database);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["datid"], "1");
    assert_eq!(rows[0]["values"]["seq_scan"], serde_json::json!(0.2));

    let indexes = stream(fixture.prepare(
        &format!(
            "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_indexes&group=object&field=idx_scan"
        ),
        None,
    ))
    .expect("four-segment index snapshot");
    let index_rates = relation_records(&indexes)
        .into_iter()
        .map(|row| {
            (
                row["key"]["datid"].as_str().unwrap().to_owned(),
                row["values"]["idx_scan"].clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(index_rates["1"], serde_json::json!(0.3));
    assert_eq!(index_rates["2"], serde_json::json!(0.2));
}

#[test]
fn events_occurrences_are_half_open_globally_limited_and_keep_physical_order() {
    let mut fixture = Fixture::new();
    let from = SEGMENT_ID + 10;
    let to_exclusive = SEGMENT_ID + 14;
    fixture.append_log_error(from - 1);
    for at in from..=to_exclusive {
        fixture.append_log_error(at);
    }
    fixture.append_log_error(to_exclusive + 1);
    fixture.finish();

    let target = format!(
        "/api/events?from={from}&to={to_exclusive}&source=pg_log_errors&representation=occurrences&limit=3"
    );
    let limited = stream(fixture.prepare(&target, None)).expect("stream event occurrences");
    assert_eq!(limited[0]["record"], "events");
    assert_eq!(limited[0]["representation"], "occurrences");
    assert_eq!(limited[0]["truncated"], true);
    let occurrences = limited
        .iter()
        .filter(|record| record["record"] == "event_occurrence")
        .collect::<Vec<_>>();
    let details = occurrences
        .iter()
        .map(|occurrence| event_detail(&fixture, occurrence))
        .collect::<Vec<_>>();
    assert_eq!(
        details
            .iter()
            .map(|record| record["at"].as_str().expect("event timestamp"))
            .collect::<Vec<_>>(),
        (from..from + 3)
            .map(|at| at.to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        limited
            .iter()
            .all(|record| record.get("next_from").is_none())
    );
    for occurrence in &occurrences {
        assert_eq!(occurrence["source"], "pg_log_errors");
        assert_eq!(occurrence["source_file"], "fixture");
        assert_eq!(occurrence["pattern"], "fixture");
        assert!(occurrence.get("sample").is_none());
        assert!(occurrence.get("segment_id").is_none());
        assert!(occurrence.get("row_ordinal").is_none());
        assert!(occurrence.get("type_id").is_none());
        assert!(occurrence.get("detail_locator").is_none());
        assert!(
            occurrence["detail_ref"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }
    for detail in &details {
        assert_eq!(detail["record"], "row_detail");
        assert_eq!(detail["section"], "pg_log_errors");
        assert_eq!(detail["fields"]["sample"]["stored_text"], "fixture");
        assert_eq!(detail["fields"]["sample"]["truncated"], false);
        for hidden in ["segment_id", "type_id", "row_ordinal", "detail_locator"] {
            assert!(detail.get(hidden).is_none(), "detail exposed {hidden}");
            assert!(
                detail["fields"].get(hidden).is_none(),
                "fields exposed {hidden}"
            );
        }
    }

    let target = format!(
        "/api/events?from={from}&to={to_exclusive}&source=pg_log_errors&representation=occurrences&limit=5000"
    );
    let full = stream(fixture.prepare(&target, None)).expect("stream full event window");
    let timestamps = full
        .iter()
        .filter(|record| record["record"] == "event_occurrence")
        .map(|record| event_detail(&fixture, record))
        .map(|record| record["at"].as_str().expect("event timestamp").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        timestamps,
        (from..to_exclusive)
            .map(|at| at.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(full[0]["truncated"], false);
}

#[test]
fn snapshot_quantity_filter_matches_a_cumulative_database_counter() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_database_rows(&[(100, 100, 10, 0), (200, 180, 30, 7)]);
    fixture.finish();

    let target = |threshold| {
        format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_database&field=datid&field=deadlocks&by=datid&search=deadlocks%3E{threshold}"
        )
    };
    let matching = stream(fixture.prepare(&target(0), None)).expect("matching snapshot");
    let rows = row_records(&matching);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], 73);

    let below_threshold =
        stream(fixture.prepare(&target(100), None)).expect("nonmatching snapshot");
    assert!(row_records(&below_threshold).is_empty());
}

#[test]
fn relation_snapshot_and_history_cross_the_bounded_chunk_without_loss() {
    const OBJECTS: u32 = 513;
    let mut fixture = Fixture::new();
    fixture.append_large_named_table_snapshots(OBJECTS);
    fixture.finish();

    let snapshot = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&group=object&field=seq_scan&page_size=1000&where.datid=1"
        ),
        None,
    ))
    .expect("relation snapshot across a chunk boundary");
    let snapshot_rows = relation_records(&snapshot);
    assert_eq!(snapshot_rows.len(), OBJECTS as usize);
    assert!(
        snapshot_rows
            .iter()
            .all(|row| row["values"]["seq_scan"] == 100_000.0)
    );

    let history = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=1",
        None,
    ))
    .expect("relation history across a chunk boundary");
    let history_rows = relation_records(&history);
    assert_eq!(history_rows.len(), 1);
    assert_eq!(history_rows[0]["values"]["seq_scan"], 51_300_000.0);
}

#[test]
fn relation_group_history_finds_the_requested_database_predecessor() {
    let (fixture, _current_segment) = four_segment_relation_fixture();

    let records = stream(fixture.prepare(
        "/api/hour?from=200000000&to=200000000&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=1",
        None,
    ))
    .expect("four-segment database history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["datid"], "1");
    assert_eq!(rows[0]["values"]["seq_scan"], serde_json::json!(0.2));
    assert_eq!(rows[0]["sample_from"], "100000000");
    assert_eq!(rows[0]["sample_to"], "200000000");
}

#[test]
fn relation_group_history_ignores_unrelated_segment_minimums_when_ordering_samples() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100, 1, 11, 10, "db", "public", "orders")]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_log_error(50);
    fixture.append_named_table_snapshots(&[(200, 1, 11, 30, "db", "public", "orders")]);
    fixture.finish();

    let records = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=1",
        None,
    ))
    .expect("database history with an unrelated older row");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["seq_scan"], serde_json::json!(200_000.0));
    assert_eq!(rows[0]["sample_from"], "100");
    assert_eq!(rows[0]["sample_to"], "200");
}

#[test]
fn relation_group_history_composes_split_logical_snapshots_across_segments() {
    let mut fixture = Fixture::new();
    fixture.append_dml_table_snapshots(&[(100, 1, 11, [0, 0, 0, 0], "db", "public", "first")]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_dml_table_snapshots(&[(100, 1, 12, [0, 0, 0, 0], "db", "public", "second")]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_dml_table_snapshots(&[(200, 1, 11, [3, 0, 0, 0], "db", "public", "first")]);
    fixture.finish_and_continue(SEGMENT_ID + 3_000);
    fixture.append_dml_table_snapshots(&[(200, 1, 12, [0, 7, 0, 7], "db", "public", "second")]);
    fixture.finish();

    let records = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=table_count&field=dml_total&field=insert_share_pct&field=hot_pct&where.datid=1",
        None,
    ))
    .expect("split relation history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["table_count"], "2");
    assert_eq!(rows[0]["values"]["dml_total"], 100_000.0);
    assert_eq!(rows[0]["values"]["insert_share_pct"], 30.0);
    assert_eq!(rows[0]["values"]["hot_pct"], 100.0);
    assert_eq!(rows[0]["sample_from"], "100");
    assert_eq!(rows[0]["sample_to"], "200");
}

#[test]
fn index_snapshot_never_borrows_a_database_or_layout_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(
        &[],
        &[
            (10_000_000, 1, 88, 100),
            (30_000_000, 1, 88, 50),
            (20_000_000, 2, 88, 10),
            (25_000_000, 2, 88, 20),
            (27_000_000, 3, 88, 100),
            (10_000_000, 4, 88, 1),
        ],
        &[(30_000_000, 4, 88, 11)],
    );
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_indexes&field=datid&field=indexrelid&field=idx_scan"
    );
    let records = stream(fixture.prepare(&target, None)).expect("index snapshot");
    let rows = row_records(&records)
        .into_iter()
        .map(|row| {
            (
                row["values"][0].as_u64().expect("database oid"),
                (row["type_id"].clone(), row["values"][2].clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[&1].1, Value::Null, "a reset has no rate");
    assert_eq!(rows[&2].1, serde_json::json!(2.0));
    assert_eq!(rows[&3].1, Value::Null, "a missing predecessor stays null");
    assert_eq!(
        rows[&4],
        (
            PgStatUserIndexesV2::CONTRACT
                .type_id
                .get()
                .to_string()
                .into(),
            Value::Null,
        ),
        "an older physical layout is not used as the predecessor"
    );
}

#[test]
fn relation_groups_keep_database_scope_and_sum_staggered_rates() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[
        (10_000_000, 1, 11, 10, "first", "public", "orders"),
        (30_000_000, 1, 11, 30, "first", "public", "orders"),
        (20_000_000, 2, 21, 5, "second", "public", "orders"),
        (25_000_000, 2, 21, 15, "second", "public", "orders"),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_tables&field=table_count&field=seq_scan&by=seq_scan&direction=desc"
    );
    let databases =
        stream(fixture.prepare(&format!("{base}&group=database"), None)).expect("database groups");
    let rows = relation_records(&databases);
    assert_eq!(
        rows.len(),
        2,
        "the root has database rows, not a global total"
    );
    assert_eq!(rows[0]["key"]["datid"], "2");
    assert_eq!(rows[0]["values"]["table_count"], "1");
    assert_eq!(rows[0]["values"]["seq_scan"], 2.0);
    assert!(rows.iter().all(|row| row["source"].is_null()));

    let schemas =
        stream(fixture.prepare(&format!("{base}&group=schema"), None)).expect("schema groups");
    let rows = relation_records(&schemas);
    assert_eq!(rows.len(), 2, "same-named schemas stay database-scoped");
    assert!(rows.iter().all(|row| row["key"]["schemaname"] == "public"));

    let objects =
        stream(fixture.prepare(&format!("{base}&group=object"), None)).expect("object groups");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["key"]["relid"], "21");
    assert_eq!(rows[0]["key"]["relname"], "orders");
    assert!(rows[0]["source"]["ordinal"].is_string());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the cluster-wide fixture checks the full filter group sort and page contract together"
)]
fn tablespace_snapshot_groups_cluster_wide_by_oid_after_full_filtering() {
    let mut fixture = Fixture::new();
    fixture.append_placed_table_snapshots(&[
        (
            100_000_000,
            1,
            11,
            10,
            "first",
            "public",
            "orders",
            Some(5_000),
            Some("fast"),
            100,
            Some(10),
        ),
        (
            100_000_000,
            1,
            12,
            0,
            "first",
            "public",
            "items",
            Some(5_000),
            Some("fast"),
            200,
            None,
        ),
        (
            100_000_000,
            1,
            13,
            0,
            "first",
            "public",
            "archive",
            Some(6_000),
            Some("fast"),
            400,
            None,
        ),
        (
            150_000_000,
            2,
            21,
            5,
            "second",
            "sales",
            "events",
            Some(5_000),
            Some("old_fast"),
            300,
            None,
        ),
        (
            300_000_000,
            1,
            11,
            30,
            "first",
            "public",
            "orders",
            Some(5_000),
            Some("fast"),
            100,
            Some(10),
        ),
        (
            300_000_000,
            1,
            12,
            20,
            "first",
            "public",
            "items",
            Some(5_000),
            Some("fast"),
            200,
            None,
        ),
        (
            300_000_000,
            1,
            13,
            10,
            "first",
            "public",
            "archive",
            Some(6_000),
            Some("fast"),
            400,
            None,
        ),
        (
            300_000_000,
            1,
            14,
            0,
            "first",
            "public",
            "partitioned",
            None,
            None,
            0,
            None,
        ),
        (
            250_000_000,
            2,
            21,
            15,
            "second",
            "sales",
            "events",
            Some(5_000),
            Some("renamed_fast"),
            300,
            None,
        ),
    ]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=300000000&section=pg_stat_user_tables&group=tablespace&field=tablespace&field=table_count&field=displayed_storage_bytes&field=seq_scan&by=displayed_storage_bytes&direction=desc"
    );
    let records = stream(fixture.prepare(&target, None)).expect("tablespace groups");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 2, "same names do not merge different OIDs");
    assert_eq!(
        rows[0]["key"],
        serde_json::json!({"tablespace_oid": "5000"})
    );
    assert_eq!(rows[0]["values"]["tablespace"], "fast");
    assert_eq!(rows[0]["values"]["table_count"], "3");
    assert_eq!(rows[0]["values"]["displayed_storage_bytes"], "610");
    assert_eq!(rows[0]["values"]["seq_scan"], 0.3);
    assert_eq!(rows[1]["key"]["tablespace_oid"], "6000");
    assert!(rows.iter().all(|row| row["source"].is_null()));

    let filtered = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=300000000&section=pg_stat_user_tables&group=tablespace&field=table_count&field=displayed_storage_bytes&where.tablespace_oid=5000"
        ),
        None,
    ))
    .expect("OID-filtered tablespace group");
    let filtered = relation_records(&filtered);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["key"]["tablespace_oid"], "5000");
    assert_eq!(filtered[0]["values"]["table_count"], "3");

    let searched = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=300000000&section=pg_stat_user_tables&group=tablespace&field=tablespace&field=table_count&search=archive"
        ),
        None,
    ))
    .expect("search before tablespace grouping");
    let searched = relation_records(&searched);
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0]["key"]["tablespace_oid"], "6000");
    assert_eq!(searched[0]["values"]["table_count"], "1");

    let compared = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=300000000&section=pg_stat_user_tables&group=tablespace&field=table_count&search=size%3E500B"
        ),
        None,
    ))
    .expect("comparison after tablespace reduction");
    let compared_rows = relation_records(&compared);
    assert_eq!(compared_rows.len(), 1);
    assert_eq!(compared_rows[0]["key"]["tablespace_oid"], "5000");
    assert_eq!(compared_rows[0]["values"]["table_count"], "3");
    let page = compared
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("tablespace comparison trailer");
    assert_eq!(page["eligible"], "1");
}

#[test]
fn index_tablespaces_use_each_index_placement_and_keep_missing_labels() {
    let mut fixture = Fixture::new();
    fixture.append_placed_index_snapshots(&[
        (
            100_000_000,
            1,
            101,
            0,
            "db",
            "public",
            "orders",
            "orders_heap_idx",
            7_000,
            Some("heap_ts"),
            100,
        ),
        (
            100_000_000,
            1,
            102,
            0,
            "db",
            "public",
            "orders",
            "orders_fast_idx",
            8_000,
            None,
            200,
        ),
        (
            200_000_000,
            1,
            101,
            10,
            "db",
            "public",
            "orders",
            "orders_heap_idx",
            7_000,
            Some("heap_ts"),
            100,
        ),
        (
            200_000_000,
            1,
            102,
            20,
            "db",
            "public",
            "orders",
            "orders_fast_idx",
            8_000,
            None,
            200,
        ),
    ]);
    fixture.finish();

    let records = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200000000&section=pg_stat_user_indexes&group=tablespace&field=tablespace&field=index_count&field=main_fork_bytes&field=idx_scan&by=main_fork_bytes&direction=desc"
        ),
        None,
    ))
    .expect("index tablespace groups");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["key"]["tablespace_oid"], "8000");
    assert_eq!(rows[0]["values"]["tablespace"], Value::Null);
    assert_eq!(rows[0]["values"]["main_fork_bytes"], "200");
    assert_eq!(rows[0]["values"]["idx_scan"], 0.2);
    assert_eq!(rows[1]["key"]["tablespace_oid"], "7000");
    assert_eq!(rows[1]["values"]["tablespace"], "heap_ts");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the staggered fixture keeps independent database snapshots and expected points together"
)]
fn tablespace_history_is_exact_across_staggered_database_snapshots_and_moves() {
    let mut fixture = Fixture::new();
    fixture.append_placed_table_snapshots(&[
        (
            0,
            1,
            11,
            0,
            "first",
            "public",
            "orders",
            Some(5_000),
            Some("fast"),
            100,
            None,
        ),
        (
            10_000_000,
            2,
            21,
            0,
            "second",
            "public",
            "events",
            Some(5_000),
            Some("old_fast"),
            200,
            None,
        ),
        (
            100_000_000,
            1,
            11,
            10,
            "first",
            "public",
            "orders",
            Some(5_000),
            Some("fast"),
            100,
            None,
        ),
        (
            110_000_000,
            2,
            21,
            10,
            "second",
            "public",
            "events",
            Some(5_000),
            Some("old_fast"),
            200,
            None,
        ),
        (
            200_000_000,
            1,
            11,
            30,
            "first",
            "public",
            "orders",
            Some(5_000),
            Some("fast"),
            100,
            None,
        ),
        (
            250_000_000,
            2,
            21,
            38,
            "second",
            "public",
            "events",
            Some(5_000),
            Some("renamed_fast"),
            200,
            None,
        ),
        (
            300_000_000,
            1,
            11,
            50,
            "first",
            "public",
            "orders",
            Some(6_000),
            Some("archive"),
            100,
            None,
        ),
    ]);
    fixture.finish();

    let records = stream(fixture.prepare(
        "/api/hour?from=200000000&to=300000000&section=pg_stat_user_tables&group=tablespace&field=tablespace&field=table_count&field=main_fork_bytes&field=seq_scan&where.tablespace_oid=5000",
        None,
    ))
    .expect("cross-database tablespace history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["source"], Value::Null);
    assert_eq!(rows[0]["sample_to"], "200000000");
    assert_eq!(rows[0]["values"]["table_count"], "2");
    assert_eq!(rows[0]["values"]["main_fork_bytes"], "300");
    assert_eq!(rows[0]["values"]["seq_scan"], 0.3);
    assert_eq!(rows[0]["values"]["tablespace"], "fast");
    assert_eq!(rows[1]["sample_to"], "250000000");
    assert_eq!(rows[1]["values"]["seq_scan"], 0.4);
    assert_eq!(rows[1]["values"]["tablespace"], "renamed_fast");
    assert_eq!(rows[2]["sample_to"], "300000000");
    assert_eq!(rows[2]["values"]["table_count"], "1");
    assert_eq!(rows[2]["values"]["main_fork_bytes"], "200");
    assert_eq!(rows[2]["values"]["seq_scan"], 0.2);
}

#[test]
fn relation_group_history_reuses_exact_reducers_across_segments_and_the_full_set() {
    let mut fixture = Fixture::new();
    let mut previous = (1..=205)
        .map(|relid| (100, 1, relid, [0, 0, 0, 0], "db", "public", "table"))
        .collect::<Vec<_>>();
    previous.push((100, 2, 1, [0, 0, 0, 0], "other", "public", "table"));
    fixture.append_dml_table_snapshots(&previous);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);

    let mut current = (1..=205)
        .map(|relid| {
            let counters = if relid == 1 {
                [10, 0, 0, 0]
            } else {
                [0, 1, 0, 1]
            };
            (200, 1, relid, counters, "db", "public", "table")
        })
        .collect::<Vec<_>>();
    current.push((200, 2, 1, [9_999, 0, 0, 0], "other", "public", "table"));
    fixture.append_dml_table_snapshots(&current);
    fixture.finish();

    let target = "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=schema&field=table_count&field=dml_total&field=insert_share_pct&where.datid=1&where.schemaname=public";
    let records = stream(fixture.prepare(target, None)).expect("grouped relation history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["datid"], "1");
    assert_eq!(rows[0]["key"]["schemaname"], "public");
    assert_eq!(rows[0]["values"]["table_count"], "205");
    assert_eq!(rows[0]["values"]["dml_total"], 2_140_000.0);
    let insert_share = rows[0]["values"]["insert_share_pct"]
        .as_f64()
        .expect("insert share");
    assert!((insert_share - 500.0 / 107.0).abs() < 1e-12);
    assert_eq!(rows[0]["sample_from"], "100");
    assert_eq!(rows[0]["sample_to"], "200");
    assert!(rows[0]["source"].is_null());

    let one_metric = "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=dml_total&where.datid=1";
    let records = stream(fixture.prepare(one_metric, None)).expect("single-metric grouped history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["dml_total"], 2_140_000.0);
}

#[test]
fn relation_group_history_keeps_zero_reset_and_missing_predecessor_distinct() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 10, "db", "steady", "steady_table"),
        (100, 1, 12, 20, "db", "reset", "reset_table"),
    ]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_named_table_snapshots(&[
        (200, 1, 11, 10, "db", "steady", "steady_table"),
        (200, 1, 12, 5, "db", "reset", "reset_table"),
        (200, 1, 13, 1, "db", "new", "new_table"),
    ]);
    fixture.finish();

    let load_schema = |schema: &str| {
        let target = format!(
            "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=schema&field=seq_scan&where.datid=1&where.schemaname={schema}"
        );
        let records = stream(fixture.prepare(&target, None)).expect("schema history");
        relation_records(&records)[0]["values"]["seq_scan"].clone()
    };
    assert_eq!(load_schema("steady"), serde_json::json!(0.0));
    assert_eq!(load_schema("reset"), Value::Null);
    assert_eq!(load_schema("new"), Value::Null);

    let database = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=1",
        None,
    ))
    .expect("database history");
    assert_eq!(
        relation_records(&database)[0]["values"]["seq_scan"],
        Value::Null,
        "one reset or missing object predecessor makes the exact database aggregate unavailable",
    );
}

#[test]
fn active_wal_relation_history_handles_snapshot_ordered_parts() {
    let mut fixture = Fixture::new();
    fixture.append_dml_table_snapshots(&[
        (100, 1, 11, [0, 0, 0, 0], "db", "public", "first"),
        (100, 1, 12, [0, 0, 0, 0], "db", "public", "second"),
    ]);
    fixture.append_dml_table_snapshots(&[
        (200, 1, 11, [3, 0, 0, 0], "db", "public", "first"),
        (200, 1, 12, [0, 7, 0, 7], "db", "public", "second"),
    ]);

    let records = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=table_count&field=dml_total&field=insert_share_pct&where.datid=1",
        None,
    ))
    .expect("active WAL grouped history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["table_count"], "2");
    assert_eq!(rows[0]["values"]["dml_total"], 100_000.0);
    assert_eq!(rows[0]["values"]["insert_share_pct"], 30.0);
    assert_eq!(rows[0]["sample_from"], "100");
    assert_eq!(rows[0]["sample_to"], "200");
}

#[test]
fn relation_derivatives_sort_the_full_set_and_recompute_group_ratios() {
    let mut fixture = Fixture::new();
    fixture.append_dml_table_snapshots(&[
        (100, 1, 11, [0, 0, 0, 0], "db", "public", "small"),
        (100, 1, 12, [0, 0, 0, 0], "db", "public", "mostly_insert"),
        (100, 1, 13, [0, 0, 0, 0], "db", "public", "mostly_update"),
        (200, 1, 11, [1, 0, 0, 0], "db", "public", "small"),
        (200, 1, 12, [9, 1, 0, 1], "db", "public", "mostly_insert"),
        (200, 1, 13, [0, 90, 0, 90], "db", "public", "mostly_update"),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&field=dml_total&field=insert_share_pct&by=derived.dml_total&direction=desc"
    );
    let objects = stream(fixture.prepare(&format!("{base}&group=object&page_size=1"), None))
        .expect("derived relation page");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["relid"], "13");
    assert_eq!(rows[0]["values"]["dml_total"], 900_000.0);
    let page = objects
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("derived page trailer");
    assert_eq!(page["eligible"], "3");
    assert_eq!(page["has_more"], true);

    let databases = stream(fixture.prepare(&format!("{base}&group=database"), None))
        .expect("derived database group");
    let rows = relation_records(&databases);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["dml_total"], 1_010_000.0);
    let share = rows[0]["values"]["insert_share_pct"]
        .as_f64()
        .expect("aggregate insert share");
    assert!((share - 1_000.0 / 101.0).abs() < 1e-12);
}

#[test]
fn relation_search_runs_before_group_sort_and_page() {
    let mut fixture = Fixture::new();
    let mut rows = Vec::new();
    for relid in 1..=205 {
        let name = if relid == 1 {
            "needle_outside_unfiltered_page"
        } else {
            "ordinary"
        };
        rows.push((100, 1, relid, 0, "db", "public", name));
        rows.push((200, 1, relid, i64::from(relid), "db", "public", name));
    }
    fixture.append_named_table_snapshots(&rows);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&group=object&field=seq_scan&by=seq_scan&page_size=200&search=table_name%3Aneedle_outside_unfiltered_page%20AND%20schema%3Apublic"
    );
    let records = stream(fixture.prepare(&target, None)).expect("searched relation page");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["relid"], "1");
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("relation trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture proves grouped comparison behavior across every reducer phase"
)]
fn relation_comparison_filters_reduced_hidden_metrics_before_page() {
    const MB: i64 = 1_000_000;
    let mut fixture = Fixture::new();
    fixture.append_placed_table_snapshots(&[
        (
            100,
            1,
            1,
            0,
            "db",
            "pair",
            "first",
            Some(1663),
            Some("pg_default"),
            50 * MB,
            Some(10 * MB),
        ),
        (
            100,
            1,
            2,
            0,
            "db",
            "pair",
            "second",
            Some(1663),
            Some("pg_default"),
            30 * MB,
            Some(30 * MB),
        ),
        (
            100,
            1,
            3,
            0,
            "db",
            "boundary",
            "exact",
            Some(1663),
            Some("pg_default"),
            100 * MB,
            None,
        ),
        (
            100,
            1,
            999,
            0,
            "db",
            "large",
            "outside_page",
            Some(1664),
            Some("fast"),
            150 * MB,
            None,
        ),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_user_tables&field=table_count&page_size=1"
    );
    let objects =
        stream(fixture.prepare(&format!("{base}&group=object&search=size%3E100MB"), None))
            .expect("hidden object size comparison");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["relid"], "999");
    let page = objects
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("object comparison trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");

    let object_or = stream(fixture.prepare(
        &format!("{base}&group=object&search=table_name%3Aexact%20OR%20size%3E100MB"),
        None,
    ))
    .expect("object member or metric OR");
    let object_or_page = object_or
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("object OR trailer");
    assert_eq!(object_or_page["eligible"], "2");
    assert_eq!(object_or_page["returned"], "1");

    let grouped = stream(fixture.prepare(
        &format!("{base}&group=schema&search=schema%3Apair%20AND%20size%3E100MB"),
        None,
    ))
    .expect("post-reducer schema size comparison");
    let rows = relation_records(&grouped);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["schemaname"], "pair");
    assert_eq!(rows[0]["values"]["table_count"], "2");
    let layout = grouped
        .iter()
        .find(|record| record["record"] == "relation_layout")
        .expect("relation comparison layout");
    assert_eq!(layout["columns"].as_array().map(Vec::len), Some(1));
    assert_eq!(layout["columns"][0]["name"], "table_count");

    let pre_or = stream(fixture.prepare(
        &format!("{base}&group=schema&search=%28schema%3Apair%20OR%20schema%3Aboundary%29%20AND%20size%3E100MB"),
        None,
    ))
    .expect("pre-reducer grouped OR");
    let rows = relation_records(&pre_or);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["schemaname"], "pair");
    assert_eq!(rows[0]["values"]["table_count"], "2");

    let post_or = stream(fixture.prepare(
        &format!("{base}&group=schema&search=schema%3Apair%20AND%20%28size%3E120MB%20OR%20table_count%3E1%29"),
        None,
    ))
    .expect("post-reducer grouped OR");
    let rows = relation_records(&post_or);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["schemaname"], "pair");

    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    let query = "at=100&section=pg_stat_user_tables&group=schema&field=table_count&page_size=1&search=schema%3Apair%20OR%20size%3E100MB";
    let route = crate::route::parse(&path, Some(query)).expect("mixed grouped OR route");
    assert!(matches!(
        crate::api::prepare(fixture.root(), SOURCES, route, None),
        Err(ApiError::BadFilter(parameter)) if parameter == "search"
    ));

    for operator in ["%3E", "%3C"] {
        let boundary = stream(fixture.prepare(
            &format!("{base}&group=object&search=table_name%3Aexact%20AND%20size{operator}100MB"),
            None,
        ))
        .expect("exact size boundary comparison");
        assert!(relation_records(&boundary).is_empty());
        let page = boundary
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("boundary comparison trailer");
        assert_eq!(page["eligible"], "0");
    }
}

#[test]
fn index_definition_is_full_search_input_but_never_an_aggregate_value() {
    let mut fixture = Fixture::new();
    let current_segment = SEGMENT_ID + 1_000;
    let definition = "CREATE UNIQUE INDEX exact_fixture_idx ON public.orders USING btree (tenant_id, created_at) WHERE archived_at IS NULL";
    fixture.append_named_index_snapshots(&[
        (
            100,
            1,
            51,
            0,
            "db",
            "public",
            "orders",
            "exact_fixture_idx",
            definition,
        ),
        (
            200,
            1,
            51,
            3,
            "db",
            "public",
            "orders",
            "exact_fixture_idx",
            definition,
        ),
    ]);
    fixture.finish_and_continue(current_segment);
    fixture.append_named_index_snapshots(&[(
        900,
        1,
        51,
        9,
        "db",
        "public",
        "orders",
        "exact_fixture_idx",
        definition,
    )]);
    fixture.finish();

    let aggregate_target = format!(
        "/api/segments/{current_segment}/snapshot?at=400&section=pg_stat_user_indexes&group=database&field=index_count&search=archived_at"
    );
    let aggregate = stream(fixture.prepare(&aggregate_target, None)).expect("definition search");
    let rows = relation_records(&aggregate);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["index_count"], "1");
    assert!(rows[0]["values"].get("indexdef").is_none());
    assert!(rows[0]["source"].is_null());

    let object_target = format!(
        "/api/segments/{current_segment}/snapshot?at=400&section=pg_stat_user_indexes&group=object&field=indexrelname&field=relid&field=relname&field=idx_scan&search=archived_at"
    );
    let object = stream(fixture.prepare(&object_target, None)).expect("index object");
    let rows = relation_records(&object);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["indexrelname"], "exact_fixture_idx");
    assert_eq!(rows[0]["key"]["relid"], "50");
    assert!(rows[0]["values"].get("indexdef").is_none());

    let source = rows[0]["source"].as_object().expect("physical source");
    assert_eq!(source["segment_id"], SEGMENT_ID.to_string());
    let detail_target = format!(
        "/api/segments/{}/snapshot?at={}&section=pg_stat_user_indexes&field=indexdef&type_id={}&row_ordinal={}&text=65536",
        source["segment_id"].as_str().unwrap(),
        source["timestamp"].as_str().unwrap(),
        source["type_id"].as_str().unwrap(),
        source["ordinal"].as_str().unwrap(),
    );
    let detail = stream(fixture.prepare(&detail_target, None)).expect("exact definition detail");
    assert_eq!(row_records(&detail)[0]["values"][0], definition);
}

#[test]
fn low_activity_filters_exact_object_deltas_before_grouping_and_paging() {
    let mut fixture = Fixture::new();
    fixture.append_named_index_snapshots(&[
        (
            100,
            1,
            51,
            3,
            "db",
            "public",
            "orders",
            "inactive_idx",
            "CREATE INDEX inactive_idx ON public.orders (id)",
        ),
        (
            200,
            1,
            51,
            3,
            "db",
            "public",
            "orders",
            "inactive_idx",
            "CREATE INDEX inactive_idx ON public.orders (id)",
        ),
        (
            100,
            1,
            61,
            0,
            "db",
            "public",
            "events",
            "active_idx",
            "CREATE INDEX active_idx ON public.events (id)",
        ),
        (
            200,
            1,
            61,
            8,
            "db",
            "public",
            "events",
            "active_idx",
            "CREATE INDEX active_idx ON public.events (id)",
        ),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_indexes&field=index_count&field=idx_scan&where.no_scans=true&page_size=1"
    );
    let objects = stream(fixture.prepare(&format!("{base}&group=object"), None))
        .expect("low-activity objects");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["indexrelname"], "inactive_idx");
    assert_eq!(rows[0]["values"]["idx_scan"], 0.0);
    let page = objects
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("low-activity page");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["has_more"], false);

    let databases = stream(fixture.prepare(&format!("{base}&group=database"), None))
        .expect("low-activity database rollup");
    let rows = relation_records(&databases);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["index_count"], "1");
}

#[test]
fn an_order_needs_one_section_to_name_a_column_in() {
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    assert!(crate::route::parse(&path, Some("at=1&section=a&section=b&by=x")).is_err());
    assert!(crate::route::parse(&path, Some("at=1&section=a&section=b&page_size=5")).is_err());
    assert!(crate::route::parse(&path, Some("at=1&section=a&by=x&page_size=5")).is_ok());
}

#[test]
fn the_first_moment_of_a_segment_rates_against_the_segment_before_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");

    let first = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("first id")).expect("first");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(100, 0, 10)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(first.id, &part).expect("append first");
    write_segment(&journal, &writer, first).expect("finish first");
    journal.reset().expect("close the first segment");

    let second = SegmentAddress::new(SegmentId::new(SEGMENT_ID + 1_000).expect("second id"))
        .expect("second");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(200, 0, 30)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(second.id, &part).expect("append second");
    write_segment(&journal, &writer, second).expect("finish second");

    let target = format!(
        "/api/segments/{}/snapshot?at=200&section=os_diskstats&field=reads",
        SEGMENT_ID + 1_000
    );
    let path = target.split('?').next().expect("path");
    let query = target.split_once('?').expect("query").1;
    let route = crate::route::parse(path, Some(query)).expect("route");
    let prepared =
        crate::api::prepare(directory.path(), SOURCES, route, None).expect("prepare snapshot");
    let records = stream(prepared).expect("snapshot body");
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], serde_json::json!(200_000.0));
}

#[test]
fn string_identity_matches_across_segment_dictionaries() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");

    let first = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("first id")).expect("first");
    let mut first_interner = Interner::new(DictLimits::default());
    let first_iface = StrId(
        first_interner
            .intern(b"eth0")
            .expect("first interface")
            .get(),
    );
    let first_dictionary = dict::encode(first_interner.window()).expect("first dictionary");
    let mut first_buffers = SectionBuffers::new();
    first_buffers
        .push(netdev(100, first_iface, 10))
        .expect("first netdev row");
    let first_part = first_buffers
        .flush(&first_dictionary)
        .expect("encode first")
        .expect("first part");
    journal.append(first.id, &first_part).expect("append first");
    write_segment(&journal, &writer, first).expect("finish first");
    journal.reset().expect("close first");

    let second = SegmentAddress::new(SegmentId::new(SEGMENT_ID + 1_000).expect("second id"))
        .expect("second");
    let mut second_interner = Interner::new(DictLimits::default());
    let _unused = second_interner
        .intern(b"lo")
        .expect("another dictionary value");
    let second_iface = StrId(
        second_interner
            .intern(b"eth0")
            .expect("second interface")
            .get(),
    );
    assert_eq!(
        first_iface, second_iface,
        "str_id is the stable content hash, not a segment-local ordinal"
    );
    let second_dictionary = dict::encode(second_interner.window()).expect("second dictionary");
    let mut second_buffers = SectionBuffers::new();
    second_buffers
        .push(netdev(200, second_iface, 30))
        .expect("second netdev row");
    let second_part = second_buffers
        .flush(&second_dictionary)
        .expect("encode second")
        .expect("second part");
    journal
        .append(second.id, &second_part)
        .expect("append second");
    write_segment(&journal, &writer, second).expect("finish second");

    let target = format!(
        "/api/segments/{}/snapshot?at=200&section=os_netdev&field=iface&field=rx_bytes",
        SEGMENT_ID + 1_000
    );
    let (path, query) = target.split_once('?').expect("snapshot query");
    let route = crate::route::parse(path, Some(query)).expect("snapshot route");
    let prepared = crate::api::prepare(directory.path(), SOURCES, route, None)
        .expect("prepare netdev snapshot");
    let records = stream(prepared).expect("netdev snapshot");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!(["eth0", 200_000.0]));
}

#[test]
fn a_moment_before_the_first_sample_here_is_answered_from_the_segment_before() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");

    let first = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("first id")).expect("first");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(100, 0, 7)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(first.id, &part).expect("append first");
    write_segment(&journal, &writer, first).expect("finish first");
    journal.reset().expect("close the first segment");

    let second = SegmentAddress::new(SegmentId::new(SEGMENT_ID + 1_000).expect("second id"))
        .expect("second");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(900, 0, 9)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(second.id, &part).expect("append second");
    write_segment(&journal, &writer, second).expect("finish second");

    let path = format!("/api/segments/{}/snapshot", SEGMENT_ID + 1_000);
    let route =
        crate::route::parse(&path, Some("at=400&section=os_diskstats&field=reads")).expect("route");
    let prepared =
        crate::api::prepare(directory.path(), SOURCES, route, None).expect("prepare snapshot");
    let records = stream(prepared).expect("snapshot body");
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "the earlier segment answers instead of nothing"
    );
    assert_eq!(rows[0]["timestamp"], "100");
}

#[test]
fn lifetime_cpu_time_rides_the_snapshot_beside_the_rates_it_cannot_be_derived_from() {
    let mut fixture = Fixture::new();
    fixture.append_quantitative_processes();
    fixture.finish();

    // The tree lens never asks for utime and stime, so the column has to pull
    // its own inputs into the projection.
    let alone = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=2000000&section=os_process&field=pid&field=cpu_time_ticks&by=pid&page_size=2"
        ),
        None,
    ))
    .expect("lifetime cpu time without its inputs");
    assert_eq!(
        row_records(&alone)
            .iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [serde_json::json!([6, "0"]), serde_json::json!([5, "50"]),]
    );

    let records = stream(fixture.prepare(
        &format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=2000000&section=os_process&field=pid&field=utime&field=stime&field=cpu_time_ticks&by=pid&page_size=2"
        ),
        None,
    ))
    .expect("lifetime cpu time snapshot");

    let layout = records
        .iter()
        .find(|record| record["record"] == "layout")
        .expect("process layout");
    let column = layout["layout"]["columns"]
        .as_array()
        .expect("layout columns")
        .iter()
        .find(|column| column["name"] == "cpu_time_ticks")
        .expect("lifetime cpu column");
    assert_eq!(column["type"], "i64");
    assert_eq!(column["class"], "gauge");
    assert_eq!(column["unit"], "jiffies");
    assert_eq!(layout["rates"], serde_json::json!(["utime", "stime"]));

    // Process 5 was first seen at this moment, so it has no rate to show and
    // its lifetime total is the only CPU reading the row can carry.
    assert_eq!(
        row_records(&records)
            .iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [
            serde_json::json!([6, 0.0, 0.0, "0"]),
            serde_json::json!([5, Value::Null, Value::Null, "50"]),
        ]
    );
}
