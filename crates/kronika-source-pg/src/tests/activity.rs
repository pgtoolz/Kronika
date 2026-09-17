use super::{ActivityRow, ActivityVersion, activity_query, activity_version, to_v1, to_v2, to_v3};
use crate::tests::intern as fake_intern;

fn sample_row() -> ActivityRow {
    ActivityRow {
        ts: 2_000,
        pid: 42,
        leader_pid: Some(7),
        datid: Some(16_384),
        datname: Some("app".to_owned()),
        usename: Some("alice".to_owned()),
        application_name: "psql".to_owned(),
        client_addr: String::new(),
        backend_type: "client backend".to_owned(),
        state: Some("active".to_owned()),
        wait_event_type: None,
        wait_event: None,
        query: Some("select 1".to_owned()),
        query_id: Some(123),
        backend_xid_age: Some(5),
        backend_xmin_age: Some(9),
        backend_start: 1_000,
        xact_start: Some(1_500),
        query_start: Some(1_800),
        state_change: Some(1_900),
    }
}

#[test]
fn version_follows_catalog_changes() {
    assert_eq!(activity_version(10), ActivityVersion::V1);
    assert_eq!(activity_version(12), ActivityVersion::V1);
    assert_eq!(activity_version(13), ActivityVersion::V2);
    assert_eq!(activity_version(14), ActivityVersion::V3);
    assert_eq!(activity_version(18), ActivityVersion::V3);
}

#[test]
fn query_includes_version_specific_columns() {
    assert!(!activity_query(ActivityVersion::V1).contains("leader_pid"));
    assert!(!activity_query(ActivityVersion::V1).contains("query_id"));
    assert!(activity_query(ActivityVersion::V2).contains("leader_pid"));
    assert!(!activity_query(ActivityVersion::V2).contains("query_id"));
    assert!(!activity_query(ActivityVersion::V2).contains("datid"));
    assert!(activity_query(ActivityVersion::V3).contains("leader_pid"));
    assert!(activity_query(ActivityVersion::V3).contains("query_id"));
    assert!(activity_query(ActivityVersion::V3).contains("datid"));
    for v in [
        ActivityVersion::V1,
        ActivityVersion::V2,
        ActivityVersion::V3,
    ] {
        assert!(activity_query(v).contains("pg_stat_activity"));
        assert!(activity_query(v).contains("kronika:"));
    }
}

#[test]
fn query_bounds_text_excludes_self_and_has_no_row_ceiling() {
    let query = activity_query(ActivityVersion::V3);
    assert!(query.contains("left(query, 65536) AS query"));
    assert!(query.contains("WHERE pid <> pg_catalog.pg_backend_pid()"));
    assert!(
        query.contains("application_name IS DISTINCT FROM current_setting('application_name')")
    );
    assert!(query.ends_with("ORDER BY pid"));
    assert!(!query.contains("LIMIT"));
}

#[test]
fn every_layout_excludes_all_connections_from_this_collector_process() {
    for version in [
        ActivityVersion::V1,
        ActivityVersion::V2,
        ActivityVersion::V3,
    ] {
        let query = activity_query(version);
        assert!(query.contains("pid <> pg_catalog.pg_backend_pid()"));
        assert!(
            query.contains("application_name IS DISTINCT FROM current_setting('application_name')")
        );
    }
}

#[test]
fn to_v3_maps_every_column_and_interns_strings() {
    let r = to_v3(&sample_row(), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.pid, 42);
    assert_eq!(r.leader_pid, Some(7));
    assert_eq!(r.datid, Some(16_384));
    assert_eq!(r.datname, Some(fake_intern(b"app").unwrap()));
    assert_eq!(r.application_name, fake_intern(b"psql").unwrap());
    assert_eq!(r.client_addr, fake_intern(b"").unwrap());
    assert_eq!(r.wait_event_type, None);
    assert_eq!(r.query, Some(fake_intern(b"select 1").unwrap()));
    assert_eq!(r.query_id, Some(123));
    assert_eq!(r.backend_xmin_age, Some(9));
    assert_eq!(r.xact_start.map(|t| t.0), Some(1_500));
}

#[test]
fn to_v2_keeps_leader_pid() {
    let r = to_v2(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.leader_pid, Some(7));
    assert_eq!(r.datname, Some(fake_intern(b"app").unwrap()));
    assert_eq!(r.backend_xmin_age, Some(9));
}

#[test]
fn to_v1_maps_the_base_layout() {
    let r = to_v1(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.datname, Some(fake_intern(b"app").unwrap()));
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.pid, 42);
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(to_v3(&sample_row(), |_| Err("full")), Err("full"));
}
