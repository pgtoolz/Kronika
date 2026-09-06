use std::path::{Path, PathBuf};

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_reader::{Reader, SegmentKind, SegmentRef};
use kronika_registry::instance_metadata::{InstanceMetadata, InstanceMetadataV1};
use kronika_registry::os_cgroup_memory::OsCgroupMemoryV2;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::os_topology::OsTopology;
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::pg_log::{
    PgLogAutovacuum, PgLogCheckpoints, PgLogErrors, PgLogLifecycle, PgLogLockWaits,
    PgLogSlowQueries, PgLogTempFiles,
};
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::pg_stat_archiver::PgStatArchiver;
use kronika_registry::pg_stat_database::PgStatDatabaseV4;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};

use crate::{
    BuildError, FindingKind, LoadError, SeriesBlock, finding_keys_for_sections,
    series_keys_for_sections,
};

use super::{path_of, read, resource_selected};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn address() -> SegmentAddress {
    address_at(SEGMENT_ID)
}

fn address_at(raw: i64) -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(raw).expect("segment id")).expect("address")
}

fn zms_path(root: &Path, segment: &SegmentRef) -> PathBuf {
    let address = address_at(segment.id());
    root.join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name())
}

fn resource(
    root: &Path,
    reader: &Reader,
    segment: &SegmentRef,
    logical_name: &str,
) -> Result<super::ResourceIndex, LoadError> {
    resource_selected(
        root,
        reader,
        segment,
        &series_keys_for_sections(segment.sections(), logical_name),
    )
}

fn row(ts: i64, model_name: StrId, mhz: f64) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id: 7,
        model_name,
        mhz_max: Some(mhz),
        core_id: 3,
        socket_id: 1,
        numa_node: 0,
        scope: 0,
    }
}

fn append_fixture(journal: &mut Journal) {
    let mut interner = Interner::new(DictLimits::default());
    let model_name = StrId(
        interner
            .intern(b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL")
            .expect("intern label")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(row(100, model_name, 2_000.0))
        .expect("first row");
    buffers
        .push(row(200, model_name, 2_500.0))
        .expect("second row");
    let part = buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("nonempty part");
    journal.append(address().id, &part).expect("append fixture");
}

#[derive(Clone, Copy)]
struct HealthFixture {
    boot_time: i64,
    environment: u8,
    postgres: Option<(i64, u32)>,
}

fn append_health_fixture(
    journal: &mut Journal,
    segment_id: i64,
    config: HealthFixture,
    samples: &[(i64, [Option<i64>; 3])],
) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(interner.intern(b"fixture").expect("intern label").get());
    let active = StrId(interner.intern(b"active").expect("intern state").get());
    let dictionary = dict::encode(interner.window()).expect("health dictionary");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadata {
            ts: Ts(samples.first().expect("health sample").0),
            hostname: label,
            kernel_version: label,
            environment: config.environment,
            clock_ticks_per_sec: 100,
            page_size_bytes: 4_096,
            boot_id: label,
            btime: Ts(config.boot_time),
            postgresql_enabled: config.postgres.is_some(),
            postgresql_interval_seconds: 30,
            postgresql_effective_cpus: config.postgres.map(|_| 2),
        })
        .expect("health metadata row");
    let scope = if config.environment == 0 { 0 } else { 3 };
    for &(timestamp, totals) in samples {
        for (resource, total) in totals.into_iter().enumerate() {
            let Some(some_total) = total else {
                continue;
            };
            buffers
                .push(OsPsi {
                    ts: Ts(timestamp),
                    resource: u8::try_from(resource).expect("three PSI resources"),
                    some_avg10: 0.0,
                    some_avg60: 0.0,
                    some_avg300: 0.0,
                    some_total,
                    full_avg10: None,
                    full_avg60: None,
                    full_avg300: None,
                    full_total: None,
                    scope,
                })
                .expect("PSI row");
        }
    }
    if let Some((timestamp, count)) = config.postgres {
        for pid in 0..count {
            buffers
                .push(activity_row(
                    timestamp,
                    i32::try_from(pid).expect("fixture pid"),
                    active,
                    label,
                ))
                .expect("activity row");
        }
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode health fixture")
        .expect("nonempty health fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("health segment id"),
            &part,
        )
        .expect("append health fixture");
}

#[derive(Clone, Copy)]
enum MetadataChange {
    ManyFacts,
    BootId,
    LayoutVersion,
}

fn append_changed_metadata_fixture(journal: &mut Journal, segment_id: i64, change: MetadataChange) {
    let mut interner = Interner::new(DictLimits::default());
    let first = StrId(interner.intern(b"first").expect("intern first label").get());
    let second = StrId(
        interner
            .intern(b"second")
            .expect("intern second label")
            .get(),
    );
    let active = StrId(interner.intern(b"active").expect("intern state").get());
    let dictionary = dict::encode(interner.window()).expect("metadata dictionary");
    let mut buffers = SectionBuffers::new();
    let first_row = InstanceMetadata {
        ts: Ts(segment_id),
        hostname: first,
        kernel_version: first,
        environment: 0,
        clock_ticks_per_sec: 100,
        page_size_bytes: 4_096,
        boot_id: first,
        btime: Ts(segment_id - 10),
        postgresql_enabled: false,
        postgresql_interval_seconds: 60,
        postgresql_effective_cpus: None,
    };
    match change {
        MetadataChange::ManyFacts => {
            buffers.push(first_row).expect("first metadata row");
            buffers
                .push(InstanceMetadata {
                    ts: Ts(segment_id + 1),
                    hostname: second,
                    kernel_version: second,
                    environment: 1,
                    clock_ticks_per_sec: 250,
                    page_size_bytes: 8_192,
                    boot_id: second,
                    btime: Ts(segment_id - 20),
                    postgresql_enabled: true,
                    postgresql_interval_seconds: 30,
                    postgresql_effective_cpus: Some(2),
                })
                .expect("second metadata row");
        }
        MetadataChange::BootId => {
            buffers.push(first_row).expect("first metadata row");
            buffers
                .push(InstanceMetadata {
                    ts: Ts(segment_id + 1),
                    boot_id: second,
                    ..first_row
                })
                .expect("changed boot-id metadata row");
        }
        MetadataChange::LayoutVersion => {
            buffers
                .push(InstanceMetadataV1 {
                    ts: first_row.ts,
                    hostname: first_row.hostname,
                    kernel_version: first_row.kernel_version,
                    environment: first_row.environment,
                    clock_ticks_per_sec: first_row.clock_ticks_per_sec,
                    page_size_bytes: first_row.page_size_bytes,
                    boot_id: first_row.boot_id,
                    btime: first_row.btime,
                })
                .expect("v1 metadata row");
            buffers
                .push(InstanceMetadata {
                    ts: Ts(segment_id + 1),
                    ..first_row
                })
                .expect("v2 metadata row");
        }
    }
    append_changed_metric_rows(&mut buffers, segment_id, active, second);
    let part = buffers
        .flush(&dictionary)
        .expect("encode changed metadata fixture")
        .expect("nonempty changed metadata fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("metadata segment id"),
            &part,
        )
        .expect("append changed metadata fixture");
}

fn append_changed_metric_rows(
    buffers: &mut SectionBuffers,
    segment_id: i64,
    active: StrId,
    query: StrId,
) {
    for timestamp in [segment_id, segment_id + 1] {
        for resource in 0..3 {
            buffers
                .push(OsPsi {
                    ts: Ts(timestamp),
                    resource,
                    some_avg10: 0.0,
                    some_avg60: 0.0,
                    some_avg300: 0.0,
                    some_total: timestamp - segment_id,
                    full_avg10: None,
                    full_avg60: None,
                    full_avg300: None,
                    full_total: None,
                    scope: 0,
                })
                .expect("metadata-change PSI row");
        }
    }
    for pid in 0..5 {
        buffers
            .push(activity_row(segment_id + 2, pid, active, query))
            .expect("activity row");
    }
}

fn completed_changed_metadata(change: MetadataChange) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_changed_metadata_fixture(&mut journal, SEGMENT_ID, change);
    write_segment(&journal, &writer, address()).expect("finish segment");
    journal.reset().expect("leave no active segment");
    directory
}

fn activity_row(ts: i64, pid: i32, state: StrId, query: StrId) -> PgStatActivityV3 {
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
        wait_event_type: None,
        wait_event: None,
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

fn health_values(resource: &super::ResourceIndex, series: &str) -> Vec<Option<u8>> {
    resource
        .index
        .blocks
        .iter()
        .find_map(|block| match (series, block) {
            ("os", SeriesBlock::OsHealth(points))
            | ("overall", SeriesBlock::OverallHealth(points))
            | ("postgres", SeriesBlock::PostgresHealth(points)) => {
                Some(points.iter().map(|point| point.value).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn process_row(ts: i64, read_bytes: Option<i64>, label: StrId) -> OsProcess {
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

fn statement_row(ts: i64, calls: i64, total_exec_time: f64, label: StrId) -> PgStatStatementsV2 {
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

fn append_finding_fixture(
    journal: &mut Journal,
    segment_id: i64,
    process_rows: &[(i64, Option<i64>)],
    statement_rows: &[(i64, i64, f64)],
) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(
        interner
            .intern(b"FINDING-SOURCE-TEXT-MUST-STAY-IN-ZMS")
            .expect("intern source text")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    for &(ts, read_bytes) in process_rows {
        buffers
            .push(process_row(ts, read_bytes, label))
            .expect("process row");
    }
    for &(ts, calls, total_exec_time) in statement_rows {
        buffers
            .push(statement_row(ts, calls, total_exec_time, label))
            .expect("statement row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode finding fixture")
        .expect("nonempty finding fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("fixture segment id"),
            &part,
        )
        .expect("append finding fixture");
}

fn cpu_row(ts: i64, cpu_id: i32, user: i64, idle: i64, scope: u8) -> OsCpu {
    OsCpu {
        ts: Ts(ts),
        cpu_id,
        user,
        nice: 0,
        system: 0,
        idle,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
        guest: 0,
        guest_nice: 0,
        scope,
    }
}

fn lock_row(ts: i64, pid: i32, blocked_by: Vec<i32>, label: StrId) -> PgLocksV2 {
    PgLocksV2 {
        ts: Ts(ts),
        pid,
        blocked_by,
        datid: 5,
        datname: label,
        usename: Some(label),
        application_name: label,
        client_addr: label,
        backend_type: label,
        state: Some(label),
        wait_event_type: None,
        wait_event: None,
        query: label,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: None,
        xact_start: None,
        query_start: None,
        state_change: None,
        lock_locktype: None,
        lock_mode: None,
        lock_database: None,
        lock_relation: None,
        lock_relname: None,
        lock_page: None,
        lock_tuple: None,
        lock_virtualxid: None,
        lock_transactionid: None,
        lock_classid: None,
        lock_objid: None,
        lock_objsubid: None,
        lock_target: None,
        waitstart: None,
    }
}

fn archiver_row(ts: i64, failed_count: i64) -> PgStatArchiver {
    PgStatArchiver {
        ts: Ts(ts),
        archived_count: 0,
        last_archived_wal: None,
        last_archived_time: None,
        failed_count,
        last_failed_wal: None,
        last_failed_time: None,
        stats_reset: None,
    }
}

fn cgroup_memory_row(ts: i64, cgroup_path: StrId, oom_kill: i64) -> OsCgroupMemoryV2 {
    OsCgroupMemoryV2 {
        ts: Ts(ts),
        cgroup_path,
        current: 0,
        max: None,
        anon: 0,
        file: 0,
        kernel: 0,
        slab: 0,
        shmem: 0,
        low_events: 0,
        high_events: 0,
        max_events: 0,
        oom_events: 0,
        oom_kill,
        scope: 3,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one fixture row exercises five independent boundary checks at once"
)]
fn database_v4_row(
    ts: i64,
    datid: u32,
    checksum_failures: Option<i64>,
    frozen_xid_age: Option<i64>,
    min_mxid_age: Option<i64>,
    sessions_fatal: i64,
    sessions_killed: i64,
) -> PgStatDatabaseV4 {
    PgStatDatabaseV4 {
        ts: Ts(ts),
        datid,
        datname: Some(StrId(1)),
        numbackends: Some(1),
        xact_commit: 0,
        xact_rollback: 0,
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
        deadlocks: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        stats_reset: None,
        frozen_xid_age,
        min_mxid_age,
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
        checksum_failures,
        checksum_last_failure: None,
        session_time: 0.0,
        active_time: 0.0,
        idle_in_transaction_time: 0.0,
        sessions: 0,
        sessions_abandoned: 0,
        sessions_fatal,
        sessions_killed,
        parallel_workers_to_launch: 0,
        parallel_workers_launched: 0,
    }
}

fn append_database_rows(journal: &mut Journal, segment_id: i64, rows: &[PgStatDatabaseV4]) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(interner.intern(b"DB-FIXTURE").expect("intern label").get());
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    for &row in rows {
        let mut row = row;
        row.datname = Some(label);
        buffers.push(row).expect("database row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode database fixture")
        .expect("nonempty database fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("database segment id"),
            &part,
        )
        .expect("append database fixture");
}

fn append_log_event_rows(
    buffers: &mut SectionBuffers,
    timestamp: i64,
    label: StrId,
    error_category: u8,
) {
    append_log_event_rows_2_001_to_2_003(buffers, timestamp, label, error_category);
    append_log_event_rows_2_004_to_2_007(buffers, timestamp, label);
}

fn append_log_event_rows_2_001_to_2_003(
    buffers: &mut SectionBuffers,
    timestamp: i64,
    label: StrId,
    error_category: u8,
) {
    buffers
        .push(PgLogErrors {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            severity: 0,
            category: error_category,
            sqlstate: Some(label),
            pattern: label,
            count: 1,
            sample: label,
            detail: Some(label),
            hint: Some(label),
            context: Some(label),
            statement: Some(label),
            database: Some(label),
            username: Some(label),
        })
        .expect("error row");
    buffers
        .push(PgLogCheckpoints {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            phase: 0,
            reason: Some(label),
            seconds_apart: None,
            buffers_written: None,
            write_ms: None,
            sync_ms: None,
            total_ms: None,
            distance_kb: None,
            estimate_kb: None,
            wal_added: None,
            wal_removed: None,
            wal_recycled: None,
            sync_files: None,
            longest_sync_ms: None,
            average_sync_ms: None,
        })
        .expect("checkpoint row");
    buffers
        .push(PgLogAutovacuum {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            kind: 0,
            relation: Some(label),
            index_scans: None,
            pages_removed: None,
            pages_remaining: None,
            tuples_removed: None,
            tuples_remaining: None,
            tuples_dead_not_removable: None,
            elapsed_ms: None,
            buffer_hits: None,
            buffer_misses: None,
            buffer_dirtied: None,
            avg_read_rate_mbs: None,
            avg_write_rate_mbs: None,
            cpu_user_ms: None,
            cpu_system_ms: None,
            wal_records: None,
            wal_fpi: None,
            wal_bytes: None,
        })
        .expect("autovacuum row");
}

fn append_log_event_rows_2_004_to_2_007(
    buffers: &mut SectionBuffers,
    timestamp: i64,
    label: StrId,
) {
    buffers
        .push(PgLogSlowQueries {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            pattern: label,
            sample: label,
            count: 3,
            max_duration_ms: 5_000.0,
            total_duration_ms: 99_999.0,
        })
        .expect("slow query row");
    buffers
        .push(PgLogLockWaits {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            kind: 0,
            pid: Some(41),
            lock_mode: Some(label),
            lock_target: Some(label),
            duration_ms: Some(1.0),
            holding_pids: Some(label),
            wait_queue: Some(label),
            detail: Some(label),
            context: Some(label),
            statement: Some(label),
        })
        .expect("lock wait row");
    buffers
        .push(PgLogLifecycle {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            kind: 0,
            pid: Some(41),
            signal: Some(9),
            shutdown_mode: Some(label),
            message: label,
            query_detail: Some(label),
        })
        .expect("lifecycle row");
    buffers
        .push(PgLogTempFiles {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            path: Some(label),
            size_bytes: 1,
            statement: Some(label),
        })
        .expect("temporary-file row");
}

fn append_direct_fixture(journal: &mut Journal, segment_id: i64, error_category: u8) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(
        interner
            .intern(b"EVENT-SOURCE-MESSAGE-QUERY-STATEMENT-MUST-STAY-IN-ZMS")
            .expect("intern direct source text")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    let later = segment_id + 1_000_000;
    for row in [
        cpu_row(segment_id, -1, 0, 0, 0),
        cpu_row(later, -1, 80, 20, 0),
        cpu_row(later, 0, 1, 1, 0),
        cpu_row(later, 1, 1, 1, 0),
        cpu_row(later, 2, 1, 1, 1),
    ] {
        buffers.push(row).expect("CPU row");
    }
    buffers
        .push(OsLoadavg {
            ts: Ts(later),
            load1: 4.0,
            load5: 0.0,
            load15: 0.0,
            running: 1,
            total: 1,
            scope: 0,
        })
        .expect("load row");
    buffers
        .push(OsMountinfo {
            ts: Ts(later),
            major: 8,
            minor: 1,
            mount_point: label,
            root: label,
            fstype: label,
            source: label,
            is_k8s_infra: false,
            total_bytes: Some(100),
            free_bytes: Some(10),
            total_inodes: Some(100),
            available_inodes: Some(10),
            scope: 0,
        })
        .expect("mount row");
    buffers
        .push(lock_row(later, 70, vec![], label))
        .expect("lock root row");
    buffers
        .push(lock_row(later, 71, vec![70], label))
        .expect("lock waiter row");
    buffers
        .push(archiver_row(segment_id, 2))
        .expect("archiver baseline row");
    buffers
        .push(archiver_row(later, 5))
        .expect("archiver growth row");
    buffers
        .push(cgroup_memory_row(segment_id, label, 1))
        .expect("cgroup memory baseline row");
    buffers
        .push(cgroup_memory_row(later, label, 3))
        .expect("cgroup memory growth row");
    append_log_event_rows(&mut buffers, later, label, error_category);
    let part = buffers
        .flush(&dictionary)
        .expect("encode direct fixture")
        .expect("nonempty direct fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("direct segment id"),
            &part,
        )
        .expect("append direct fixture");
}

fn only_segment(reader: &Reader, kind: SegmentKind) -> SegmentRef {
    let listing = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments(..)
        .expect("list fixture");
    let segments: Vec<_> = listing
        .segments
        .into_iter()
        .filter(|segment| segment.kind() == kind)
        .collect();
    assert_eq!(segments.len(), 1, "one segment of requested kind");
    segments.into_iter().next().expect("one segment")
}

fn health_at(root: &Path, segment_id: i64) -> super::ResourceIndex {
    let reader = Reader::open(root).expect("reader");
    let segment = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments(..)
        .expect("catalog")
        .segments
        .into_iter()
        .find(|segment| segment.id() == segment_id)
        .expect("health segment");
    resource(root, &reader, &segment, "health").expect("health resource")
}

#[test]
fn an_index_lives_beside_its_finished_segment() {
    assert_eq!(
        path_of(Path::new("/data/2026/08/08/17.zms")),
        Some(Path::new("/data/2026/08/08/17.idx").to_path_buf())
    );
}

#[test]
fn active_data_never_gets_an_index_path() {
    assert_eq!(path_of(Path::new("/data/active.wal")), None);
}

#[test]
fn health_uses_the_immediately_preceding_psi_snapshot() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let config = HealthFixture {
        boot_time: SEGMENT_ID - 1_000_000,
        environment: 0,
        postgres: None,
    };
    append_health_fixture(
        &mut journal,
        SEGMENT_ID,
        config,
        &[
            (SEGMENT_ID, [Some(0), Some(0), Some(0)]),
            (SEGMENT_ID + 1_000_000, [Some(100_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish predecessor");
    journal.reset().expect("reset after predecessor");

    let current_id = SEGMENT_ID + 2_000_000;
    append_health_fixture(
        &mut journal,
        current_id,
        config,
        &[
            (current_id, [Some(200_000), Some(0), Some(0)]),
            (current_id + 1_000_000, [Some(300_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(current_id)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    let selected = health_at(directory.path(), current_id);
    assert_eq!(health_values(&selected, "os"), [Some(90), Some(90)]);
    assert_eq!(health_values(&selected, "overall"), [Some(90), Some(90)]);
    assert!(health_values(&selected, "postgres").is_empty());
}

#[test]
fn differing_metadata_rows_suppress_metadata_dependent_values() {
    let directory = completed_changed_metadata(MetadataChange::ManyFacts);

    let selected = health_at(directory.path(), SEGMENT_ID);
    assert_eq!(health_values(&selected, "os"), [None, None]);
    assert_eq!(health_values(&selected, "overall"), [None, None]);
    assert!(health_values(&selected, "postgres").is_empty());
    let reader = Reader::open(directory.path()).expect("reader");
    let segment_ref = only_segment(&reader, SegmentKind::Finished);
    let activity = resource(directory.path(), &reader, &segment_ref, "pg_stat_activity")
        .expect("activity resource");
    assert!(activity.index.blocks.iter().any(|block| matches!(
        block,
        SeriesBlock::Findings(block)
            if block.type_id == 1_001_004 && block.findings.is_empty()
    )));
    let segment = reader.open_segment(&segment_ref).expect("open segment");
    assert_eq!(segment.rows_of(1_021_002), Some(2));
}

#[test]
fn changed_boot_id_with_the_same_boot_time_suppresses_health() {
    let directory = completed_changed_metadata(MetadataChange::BootId);

    let selected = health_at(directory.path(), SEGMENT_ID);
    assert_eq!(health_values(&selected, "os"), [None, None]);
    assert_eq!(health_values(&selected, "overall"), [None, None]);
    assert!(health_values(&selected, "postgres").is_empty());
}

#[test]
fn mixed_metadata_layouts_suppress_health() {
    let directory = completed_changed_metadata(MetadataChange::LayoutVersion);

    let selected = health_at(directory.path(), SEGMENT_ID);
    assert_eq!(health_values(&selected, "os"), [None, None]);
    assert_eq!(health_values(&selected, "overall"), [None, None]);
    assert!(health_values(&selected, "postgres").is_empty());
    let reader = Reader::open(directory.path()).expect("reader");
    let segment_ref = only_segment(&reader, SegmentKind::Finished);
    let segment = reader.open_segment(&segment_ref).expect("open segment");
    assert_eq!(segment.rows_of(1_021_001), Some(1));
    assert_eq!(segment.rows_of(1_021_002), Some(1));
}

#[test]
fn reset_and_unusable_psi_snapshots_remain_unknown() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let config = HealthFixture {
        boot_time: SEGMENT_ID - 1_000_000,
        environment: 0,
        postgres: None,
    };
    append_health_fixture(
        &mut journal,
        SEGMENT_ID,
        config,
        &[(SEGMENT_ID, [Some(200_000), Some(0), Some(0)])],
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish predecessor");
    journal.reset().expect("reset after predecessor");

    let current_id = SEGMENT_ID + 1_000_000;
    append_health_fixture(
        &mut journal,
        current_id,
        config,
        &[
            (current_id, [Some(50_000), Some(0), Some(0)]),
            (current_id + 1_000_000, [Some(150_000), Some(0), None]),
            (current_id + 2_000_000, [Some(250_000), Some(0), Some(0)]),
            (current_id + 3_000_000, [Some(350_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(current_id)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    assert_eq!(
        health_values(&health_at(directory.path(), current_id), "os"),
        [None, None, None, Some(90)]
    );
}

#[test]
fn a_different_boot_does_not_seed_os_health() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_health_fixture(
        &mut journal,
        SEGMENT_ID,
        HealthFixture {
            boot_time: 1,
            environment: 0,
            postgres: None,
        },
        &[(SEGMENT_ID, [Some(100_000), Some(0), Some(0)])],
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish predecessor");
    journal.reset().expect("reset after predecessor");

    let current_id = SEGMENT_ID + 1_000_000;
    append_health_fixture(
        &mut journal,
        current_id,
        HealthFixture {
            boot_time: 2,
            environment: 0,
            postgres: None,
        },
        &[
            (current_id, [Some(200_000), Some(0), Some(0)]),
            (current_id + 1_000_000, [Some(300_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(current_id)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    assert_eq!(
        health_values(&health_at(directory.path(), current_id), "os"),
        [None, Some(90)]
    );
}

#[test]
fn overall_uses_fresh_predecessor_postgres_without_copying_its_point() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let boot_time = SEGMENT_ID - 1_000_000;
    append_health_fixture(
        &mut journal,
        SEGMENT_ID,
        HealthFixture {
            boot_time,
            environment: 0,
            postgres: Some((SEGMENT_ID + 1_000_000, 5)),
        },
        &[
            (SEGMENT_ID, [Some(0), Some(0), Some(0)]),
            (SEGMENT_ID + 1_000_000, [Some(100_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish predecessor");
    journal.reset().expect("reset after predecessor");

    let current_id = SEGMENT_ID + 2_000_000;
    append_health_fixture(
        &mut journal,
        current_id,
        HealthFixture {
            boot_time,
            environment: 0,
            postgres: Some((current_id + 500_000, 4)),
        },
        &[
            (current_id, [Some(200_000), Some(0), Some(0)]),
            (current_id + 1_000_000, [Some(300_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(current_id)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    let selected = health_at(directory.path(), current_id);
    assert_eq!(health_values(&selected, "os"), [Some(90), Some(90)]);
    assert_eq!(health_values(&selected, "overall"), [Some(70), Some(90)]);
    assert_eq!(health_values(&selected, "postgres"), [Some(100)]);
}

#[test]
fn activity_series_and_finding_share_the_same_active_snapshot() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_health_fixture(
        &mut journal,
        SEGMENT_ID,
        HealthFixture {
            boot_time: SEGMENT_ID - 1_000_000,
            environment: 0,
            postgres: Some((SEGMENT_ID + 1_000_000, 5)),
        },
        &[(SEGMENT_ID, [Some(0), Some(0), Some(0)])],
    );
    write_segment(&journal, &writer, address()).expect("finish segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let segment = only_segment(&reader, SegmentKind::Finished);
    let selected =
        resource(directory.path(), &reader, &segment, "pg_stat_activity").expect("activity index");
    let active = selected
        .index
        .blocks
        .iter()
        .find_map(|block| match block {
            SeriesBlock::PgActiveBackends { points, .. } => Some(points),
            _ => None,
        })
        .expect("active series");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].timestamp, SEGMENT_ID + 1_000_000);
    assert_eq!(active[0].count, 5);
    let finding = selected
        .index
        .blocks
        .iter()
        .find_map(|block| match block {
            SeriesBlock::Findings(block) if block.type_id == 1_001_004 => block.findings.first(),
            _ => None,
        })
        .expect("active overload finding");
    assert_eq!(finding.kind, FindingKind::KnownBad);
    assert_eq!(finding.field_ordinal, 8);
    assert_eq!(finding.row_ordinal, 0);
    assert_eq!(finding.timestamp, SEGMENT_ID + 1_000_000);
}

#[test]
fn unusable_nearest_inputs_block_older_health_values() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let boot_time = SEGMENT_ID - 1_000_000;
    append_health_fixture(
        &mut journal,
        SEGMENT_ID,
        HealthFixture {
            boot_time,
            environment: 0,
            postgres: Some((SEGMENT_ID + 1_000_000, 5)),
        },
        &[
            (SEGMENT_ID, [Some(0), Some(0), Some(0)]),
            (SEGMENT_ID + 1_000_000, [Some(100_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish oldest segment");
    journal.reset().expect("reset after oldest segment");

    let middle_id = SEGMENT_ID + 2_000_000;
    append_health_fixture(
        &mut journal,
        middle_id,
        HealthFixture {
            boot_time,
            environment: 0,
            postgres: None,
        },
        &[(middle_id, [Some(200_000), Some(0), None])],
    );
    write_segment(&journal, &writer, address_at(middle_id)).expect("finish nearest segment");
    journal.reset().expect("reset after nearest segment");

    let current_id = SEGMENT_ID + 3_000_000;
    append_health_fixture(
        &mut journal,
        current_id,
        HealthFixture {
            boot_time,
            environment: 0,
            postgres: Some((current_id + 1_500_000, 4)),
        },
        &[
            (current_id, [Some(300_000), Some(0), Some(0)]),
            (current_id + 1_000_000, [Some(400_000), Some(0), Some(0)]),
            (current_id + 2_000_000, [Some(500_000), Some(0), Some(0)]),
        ],
    );
    write_segment(&journal, &writer, address_at(current_id)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    let selected = health_at(directory.path(), current_id);
    assert_eq!(health_values(&selected, "os"), [None, Some(90), Some(90)]);
    assert_eq!(health_values(&selected, "overall"), [None, None, Some(90)]);
    assert_eq!(health_values(&selected, "postgres"), [Some(100)]);
}

#[test]
fn real_active_and_finished_resources_are_bounded_and_atomically_cached() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_fixture(&mut journal);

    let reader = Reader::open(directory.path()).expect("reader");
    let active_ref = only_segment(&reader, SegmentKind::Active);
    let active =
        resource(directory.path(), &reader, &active_ref, "health").expect("active resource");
    assert!(!active.persisted);
    assert_eq!(active.index.checksum, None);
    assert_eq!(active.index.blocks.len(), 3);

    write_segment(&journal, &writer, address()).expect("finish segment");
    let reader = Reader::open(directory.path()).expect("finished reader");
    let finished_ref = only_segment(&reader, SegmentKind::Finished);
    let index_path =
        path_of(&zms_path(directory.path(), &finished_ref)).expect("finished index path");

    let contended_owner = data_root
        .acquire_index(LayoutLimits::default())
        .expect("hold index owner");
    let (release, held) = std::sync::mpsc::sync_channel(0);
    let local = std::thread::scope(|scope| {
        scope.spawn(move || {
            held.recv_timeout(std::time::Duration::from_secs(2))
                .expect("resource returns before the bounded holder timeout");
            drop(contended_owner);
        });
        let local = resource(directory.path(), &reader, &finished_ref, "health")
            .expect("answer without waiting out the holder");
        release.send(()).expect("release index owner");
        local
    });
    assert!(!local.persisted);
    assert!(!index_path.is_file());
    let published = resource(directory.path(), &reader, &finished_ref, "health")
        .expect("publish after the holder releases");
    assert!(published.persisted);
    assert!(index_path.is_file());
    let bytes = std::fs::read(&index_path).expect("read index bytes");
    assert!(
        !bytes
            .windows(b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL".len())
            .any(|window| window == b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL"),
        "non-identity display labels do not belong in IDX"
    );
    assert_eq!(read(&index_path).expect("read index").blocks.len(), 3);
}

#[test]
fn a_published_index_does_not_require_its_finished_source_body() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_fixture(&mut journal);
    write_segment(&journal, &writer, address()).expect("finish segment");

    let reader = Reader::open(directory.path()).expect("finished reader");
    let finished_ref = only_segment(&reader, SegmentKind::Finished);
    let source_path = zms_path(directory.path(), &finished_ref);
    let published = resource(directory.path(), &reader, &finished_ref, "health")
        .expect("publish finished index");
    assert!(published.persisted);
    journal.reset().expect("leave no active segment");

    let mut bytes = std::fs::read(&source_path).expect("read source segment");
    bytes[kronika_format::MAGIC.len()] ^= 0xff;
    std::fs::write(&source_path, bytes).expect("damage source section body");

    let reader = Reader::open(directory.path()).expect("reader after damage");
    let validated = reader.segments(..).expect("validate source bodies");
    assert!(validated.segments.is_empty());
    assert_eq!(validated.warnings.len(), 1);
    assert_eq!(
        validated.warnings[0].reason.code(),
        "invalid_zms_section_checksum"
    );

    let catalog = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments(..)
        .expect("catalog-only discovery");
    assert!(catalog.warnings.is_empty());
    assert_eq!(catalog.segments.len(), 1);
    let recovered = resource(directory.path(), &reader, &catalog.segments[0], "health")
        .expect("load published index without source body");
    assert_eq!(recovered, published);
}

#[test]
fn truncated_and_unknown_finished_indexes_are_rebuilt_in_place() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_direct_fixture(&mut journal, SEGMENT_ID, 5);
    write_segment(&journal, &writer, address()).expect("finish segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let finished = only_segment(&reader, SegmentKind::Finished);
    let published = resource(directory.path(), &reader, &finished, "pg_log_errors")
        .expect("publish current index");
    let path = path_of(&zms_path(directory.path(), &finished)).expect("index path");
    let canonical = std::fs::read(&path).expect("canonical index");

    std::fs::write(&path, &canonical[..10]).expect("truncate derived index");
    let rebuilt = resource(directory.path(), &reader, &finished, "pg_log_errors")
        .expect("rebuild truncated index");
    assert_eq!(rebuilt, published);
    assert_eq!(std::fs::read(&path).expect("rebuilt index"), canonical);

    let mut unknown = canonical.clone();
    unknown[0] ^= 1;
    std::fs::write(&path, unknown).expect("replace index magic");
    let rebuilt = resource(directory.path(), &reader, &finished, "pg_log_errors")
        .expect("rebuild unknown index");
    assert_eq!(rebuilt, published);
    assert_eq!(
        std::fs::read(path).expect("rebuilt current index"),
        canonical
    );
}

#[test]
fn previous_idx_is_rebuilt_from_recorded_cpu_for_health_and_findings() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let active = StrId(interner.intern(b"active").expect("active state").get());
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadata {
            ts: Ts(SEGMENT_ID),
            hostname: active,
            kernel_version: active,
            environment: 0,
            clock_ticks_per_sec: 100,
            page_size_bytes: 4_096,
            boot_id: active,
            btime: Ts(1),
            postgresql_enabled: true,
            postgresql_interval_seconds: 30,
            postgresql_effective_cpus: None,
        })
        .expect("metadata");
    for cpu_id in -1..8 {
        buffers
            .push(cpu_row(SEGMENT_ID, cpu_id, 0, 0, 0))
            .expect("CPU snapshot");
    }
    for pid in 1..=20 {
        buffers
            .push(activity_row(SEGMENT_ID + 1, pid, active, active))
            .expect("activity");
    }
    let part = buffers.flush(&dictionary).expect("flush").expect("part");
    journal.append(address().id, &part).expect("append");
    write_segment(&journal, &writer, address()).expect("finish");
    journal.reset().expect("reset");
    let reader = Reader::open(directory.path()).expect("reader");
    let finished = only_segment(&reader, SegmentKind::Finished);
    let health = resource(directory.path(), &reader, &finished, "health").expect("health");
    assert_eq!(health_values(&health, "postgres"), [Some(80)]);
    let path = path_of(&zms_path(directory.path(), &finished)).expect("index path");
    let canonical = std::fs::read(&path).expect("canonical bytes");
    let mut stale = crate::Index::decode(&canonical).expect("decode current format");
    assert!(
        stale.blocks.iter().any(|block| matches!(
            block,
            SeriesBlock::Findings(block) if block.type_id == 1_001_004 && block.total_hits == 1
        )),
        "recorded VM capacity produces the active-backend finding"
    );
    for block in &mut stale.blocks {
        match block {
            SeriesBlock::PostgresHealth(points) => {
                for point in points {
                    point.value = None;
                }
            }
            SeriesBlock::Findings(block) if block.type_id == 1_001_004 => {
                block.findings.clear();
                block.total_hits = 0;
            }
            _ => {}
        }
    }
    let mut stale = stale.encode().expect("valid old calculation");
    stale[..8].copy_from_slice(b"KRNIDX6\0");
    let mut checksum = kronika_format::Crc32c::new();
    checksum.update(&stale[..12]);
    checksum.update(&stale[16..]);
    stale[12..16].copy_from_slice(&checksum.finalize().to_le_bytes());
    assert_eq!(
        crate::Index::decode(&stale),
        Err(crate::IndexError::BadMagic)
    );
    assert_ne!(stale, canonical);
    for logical_name in ["health", "pg_stat_activity"] {
        std::fs::write(&path, &stale).expect("persist old calculation");
        let rebuilt = resource(directory.path(), &reader, &finished, logical_name)
            .expect("recompute previous-format cache");
        assert!(rebuilt.persisted);
        assert_eq!(std::fs::read(&path).expect("rebuilt bytes"), canonical);
        assert_eq!(
            resource(directory.path(), &reader, &finished, logical_name)
                .expect("reuse matching calculation"),
            rebuilt
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one assertion checks one exact stored finding"
)]
fn assert_direct_boundary_finding(
    root: &Path,
    reader: &Reader,
    segment: &SegmentRef,
    logical_name: &str,
    type_id: u32,
    field_ordinal: u16,
    row_ordinal: u32,
) {
    let selected = resource(root, reader, segment, logical_name).expect("direct finding resource");
    let [SeriesBlock::Findings(block)] = selected.index.blocks.as_slice() else {
        panic!("one direct finding block for {logical_name}");
    };
    assert_eq!(block.type_id, type_id);
    assert_eq!(block.total_hits, 1);
    assert!(!block.truncated);
    assert_eq!(block.findings.len(), 1);
    assert_eq!(block.findings[0].kind, FindingKind::KnownBad);
    assert_eq!(block.findings[0].category, None);
    assert_eq!(block.findings[0].field_ordinal, field_ordinal);
    assert_eq!(block.findings[0].row_ordinal, row_ordinal);
    assert_eq!(block.findings[0].timestamp, SEGMENT_ID + 1_000_000);
}

fn assert_log_event_finding(
    root: &Path,
    reader: &Reader,
    segment: &SegmentRef,
    logical_name: &str,
    type_id: u32,
) {
    let selected = resource(root, reader, segment, logical_name).expect("log event resource");
    let [SeriesBlock::Findings(block)] = selected.index.blocks.as_slice() else {
        panic!("one event locator block for {logical_name}");
    };
    assert_eq!(block.type_id, type_id);
    let expected_hits = if matches!(type_id, 2_001_001 | 2_004_001) {
        2
    } else {
        1
    };
    assert_eq!(block.total_hits, expected_hits);
    assert!(!block.truncated);
    assert_eq!(block.findings.len(), expected_hits as usize);
    assert_eq!(block.findings[0].kind, FindingKind::Event);
    assert_eq!(
        block.findings[0].category,
        (type_id == 2_001_001).then_some(5)
    );
    assert_eq!(block.findings[0].field_ordinal, 0);
    assert_eq!(block.findings[0].row_ordinal, 0);
    assert_eq!(block.findings[0].timestamp, SEGMENT_ID + 1_000_000);
    if type_id == 2_004_001 {
        assert_eq!(block.findings[1].kind, FindingKind::KnownBad);
        assert_eq!(block.findings[1].category, None);
        assert_eq!(block.findings[1].field_ordinal, 6);
        assert_eq!(block.findings[1].row_ordinal, 0);
        assert_eq!(block.findings[1].timestamp, SEGMENT_ID + 1_000_000);
    }
    if type_id == 2_001_001 {
        assert_eq!(block.findings[1].kind, FindingKind::KnownBad);
        assert_eq!(block.findings[1].category, Some(5));
        assert_eq!(block.findings[1].field_ordinal, 4);
        assert_eq!(block.findings[1].row_ordinal, 0);
        assert_eq!(block.findings[1].timestamp, SEGMENT_ID + 1_000_000);
    }
}

#[test]
fn direct_boundaries_and_log_events_use_exact_production_fields() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_direct_fixture(&mut journal, SEGMENT_ID, 5);
    write_segment(&journal, &writer, address()).expect("finish direct segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let segment = only_segment(&reader, SegmentKind::Finished);
    for (logical_name, type_id, field_ordinal, row_ordinal) in [
        ("os_cpu", 1_102_001, 5, 1),
        ("os_loadavg", 1_105_001, 1, 0),
        ("os_mountinfo", 1_112_002, 9, 0),
        ("pg_locks", 1_011_002, 2, 1),
        ("pg_stat_archiver", 1_008_001, 4, 1),
        ("os_cgroup_memory", 1_202_002, 13, 1),
    ] {
        assert_direct_boundary_finding(
            directory.path(),
            &reader,
            &segment,
            logical_name,
            type_id,
            field_ordinal,
            row_ordinal,
        );
    }

    for (logical_name, type_id) in [
        ("pg_log_errors", 2_001_001),
        ("pg_log_checkpoints", 2_002_001),
        ("pg_log_autovacuum", 2_003_001),
        ("pg_log_slow_queries", 2_004_001),
        ("pg_log_lock_waits", 2_005_002),
        ("pg_log_lifecycle", 2_006_001),
    ] {
        assert_log_event_finding(directory.path(), &reader, &segment, logical_name, type_id);
    }

    let raw = reader.open_segment(&segment).expect("finished segment");
    assert_eq!(raw.rows_of(2_007_001), Some(1));
    assert!(
        finding_keys_for_sections(segment.sections())
            .iter()
            .all(|key| key.type_id != 2_007_001)
    );
    assert!(series_keys_for_sections(segment.sections(), "pg_log_temp_files").is_empty());
    let selected = resource(directory.path(), &reader, &segment, "pg_log_temp_files")
        .expect("raw temporary-file section has no index resource");
    assert!(selected.index.blocks.is_empty());

    let path = path_of(&zms_path(directory.path(), &segment)).expect("index path");
    assert!(
        read(&path).expect("current index").blocks.iter().all(
            |block| !matches!(block, SeriesBlock::Findings(block) if block.type_id == 2_007_001)
        )
    );
    let bytes = std::fs::read(path).expect("published index");
    let copied = b"EVENT-SOURCE-MESSAGE-QUERY-STATEMENT-MUST-STAY-IN-ZMS";
    assert!(
        !bytes.windows(copied.len()).any(|window| window == copied),
        "event locators contain no source row text"
    );
}

#[test]
fn pg_stat_database_boundaries_use_exact_production_fields() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");

    // datid 1 grows every counter and crosses both wraparound ages; datid 2
    // stays flat; datid 3 has no checksums and stays just under threshold.
    append_database_rows(
        &mut journal,
        SEGMENT_ID,
        &[
            database_v4_row(SEGMENT_ID, 1, Some(3), Some(0), Some(0), 0, 0),
            database_v4_row(SEGMENT_ID, 2, Some(7), Some(0), Some(0), 0, 0),
            database_v4_row(SEGMENT_ID, 3, None, Some(0), Some(0), 0, 0),
        ],
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish prior segment");
    journal.reset().expect("reset after prior segment");

    let current_id = SEGMENT_ID + 1_000_000;
    append_database_rows(
        &mut journal,
        current_id,
        &[
            database_v4_row(
                current_id,
                1,
                Some(4),
                Some(1_600_000_000),
                Some(1_600_000_000),
                1,
                1,
            ),
            database_v4_row(current_id, 2, Some(7), Some(0), Some(0), 0, 0),
            database_v4_row(
                current_id,
                3,
                None,
                Some(1_599_999_999),
                Some(1_599_999_999),
                0,
                0,
            ),
        ],
    );
    write_segment(&journal, &writer, address_at(current_id)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let listing = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments(..)
        .expect("catalog segments");
    let current = listing
        .segments
        .into_iter()
        .find(|segment| segment.id() == current_id)
        .expect("current finished segment");
    let selected = resource(directory.path(), &reader, &current, "pg_stat_database")
        .expect("pg_stat_database resource");
    let block = selected
        .index
        .blocks
        .iter()
        .find_map(|block| match block {
            SeriesBlock::Findings(block) if block.type_id == 1_005_004 => Some(block),
            _ => None,
        })
        .expect("pg_stat_database finding block");
    assert_eq!(block.type_id, 1_005_004);
    assert_eq!(block.total_hits, 5);
    assert!(!block.truncated);
    assert_eq!(block.findings.len(), 5);
    for finding in &block.findings {
        assert_eq!(finding.kind, FindingKind::KnownBad);
        assert_eq!(finding.category, None);
        assert_eq!(
            finding.row_ordinal, 0,
            "only datid 1's row crosses a boundary"
        );
        assert_eq!(finding.timestamp, current_id);
    }
    assert_eq!(
        block
            .findings
            .iter()
            .map(|finding| finding.field_ordinal)
            .collect::<Vec<_>>(),
        [20, 21, 25, 32, 33]
    );
}

#[test]
fn archiver_and_cgroup_memory_growth_ignores_a_flat_interval() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");

    let mut interner = Interner::new(DictLimits::default());
    let cgroup_path = StrId(
        interner
            .intern(b"/kubepods/burstable/pod-fixture")
            .expect("intern cgroup path")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    let flat = SEGMENT_ID + 1_000_000;
    let grows = SEGMENT_ID + 2_000_000;
    for row in [
        archiver_row(SEGMENT_ID, 2),
        archiver_row(flat, 2),
        archiver_row(grows, 5),
    ] {
        buffers.push(row).expect("archiver row");
    }
    for row in [
        cgroup_memory_row(SEGMENT_ID, cgroup_path, 1),
        cgroup_memory_row(flat, cgroup_path, 1),
        cgroup_memory_row(grows, cgroup_path, 4),
    ] {
        buffers.push(row).expect("cgroup memory row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode fixture")
        .expect("nonempty fixture");
    journal.append(address().id, &part).expect("append fixture");
    write_segment(&journal, &writer, address()).expect("finish segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let segment = only_segment(&reader, SegmentKind::Finished);

    for (logical_name, type_id, field_ordinal) in [
        ("pg_stat_archiver", 1_008_001, 4),
        ("os_cgroup_memory", 1_202_002, 13),
    ] {
        let selected =
            resource(directory.path(), &reader, &segment, logical_name).expect("resource");
        let [SeriesBlock::Findings(block)] = selected.index.blocks.as_slice() else {
            panic!("one finding block for {logical_name}");
        };
        assert_eq!(block.type_id, type_id);
        assert_eq!(block.total_hits, 1, "the flat interval must not also fire");
        assert!(!block.truncated);
        assert_eq!(block.findings.len(), 1);
        assert_eq!(block.findings[0].kind, FindingKind::KnownBad);
        assert_eq!(block.findings[0].category, None);
        assert_eq!(block.findings[0].field_ordinal, field_ordinal);
        assert_eq!(block.findings[0].row_ordinal, 2);
        assert_eq!(block.findings[0].timestamp, grows);
    }
}

#[test]
fn production_builder_rejects_an_invalid_log_error_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_direct_fixture(&mut journal, SEGMENT_ID, 11);
    write_segment(&journal, &writer, address()).expect("finish direct segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let segment = only_segment(&reader, SegmentKind::Finished);
    let error = match resource(directory.path(), &reader, &segment, "pg_log_errors") {
        Ok(_resource) => panic!("invalid category must fail the build"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        LoadError::Build(BuildError::InvalidLogErrorCategory)
    ));
}

#[test]
fn process_and_statement_metrics_stay_out_of_finding_indexes() {
    const STEP: i64 = 5 * 60 * 1_000_000;

    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");

    let process_prior: Vec<_> = (0_i64..6)
        .map(|at| (SEGMENT_ID + at * STEP, Some(at * 300)))
        .collect();
    let statement_prior: Vec<_> = (0_i32..6)
        .map(|at| {
            (
                SEGMENT_ID + i64::from(at) * STEP,
                i64::from(at),
                f64::from(at) * 100.0,
            )
        })
        .collect();
    append_finding_fixture(&mut journal, SEGMENT_ID, &process_prior, &statement_prior);
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish prior segment");
    journal.reset().expect("reset after prior segment");

    let current_ts = SEGMENT_ID + 6 * STEP;
    let process_current = [
        (current_ts, Some(301_500)),
        (current_ts + STEP, None),
        (current_ts + 2 * STEP, Some(301_800)),
        (current_ts + 3 * STEP, Some(100)),
    ];
    let statement_current = [
        (current_ts, 6, 10_500.0),
        (current_ts + STEP, 0, 0.0),
        (current_ts + 2 * STEP, 1, 100.0),
    ];
    append_finding_fixture(
        &mut journal,
        SEGMENT_ID + 1,
        &process_current,
        &statement_current,
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID + 1)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let listing = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments(..)
        .expect("catalog segments");
    let current = listing
        .segments
        .into_iter()
        .find(|segment| segment.id() == SEGMENT_ID + 1)
        .expect("current finished segment");
    let process = resource(directory.path(), &reader, &current, "os_process")
        .expect("process index selection");
    let statements = resource(directory.path(), &reader, &current, "pg_stat_statements")
        .expect("statement index selection");
    assert!(process.persisted);
    assert!(statements.persisted);
    assert_eq!(process.index.checksum, statements.index.checksum);
    assert!(process.index.blocks.is_empty());
    assert!(statements.index.blocks.is_empty());
    assert!(
        finding_keys_for_sections(current.sections())
            .iter()
            .all(|key| { !matches!(key.type_id, 1_100_001 | 1_002_001..=1_002_006) })
    );

    let raw = reader.open_segment(&current).expect("open current segment");
    assert_eq!(
        raw.rows_of(1_100_001),
        Some(u64::try_from(process_current.len()).expect("small process fixture"))
    );
    assert_eq!(
        raw.rows_of(1_002_002),
        Some(u64::try_from(statement_current.len()).expect("small statement fixture"))
    );

    let index_path = path_of(&zms_path(directory.path(), &current)).expect("finished index path");
    assert!(read(&index_path).expect("read published index").blocks.iter().all(
        |block| !matches!(block, SeriesBlock::Findings(block) if matches!(block.type_id, 1_100_001 | 1_002_001..=1_002_006))
    ));
}
