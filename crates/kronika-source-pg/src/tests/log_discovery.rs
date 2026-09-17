use super::{
    ConnectionTarget, InvalidConnection, POSTGRES_LOG_FACTS_QUERY, POSTGRES_SYSTEM_IDENTIFIER_QUERY,
};

fn target(raw: &str) -> ConnectionTarget {
    ConnectionTarget::parse(raw, 7).expect("parse connection")
}

#[test]
fn keyword_quoting_and_escaping_do_not_reach_the_label() {
    let raw =
        r"host='db\ host' user='mon\'itor' password='KW_SECRET with spaces' dbname='DB_SECRET'";
    let parsed = target(raw);

    assert_eq!(parsed.label(), "mon'itor@db host:5432");
    let debug = format!("{parsed:?}");
    for secret in ["KW_SECRET", "DB_SECRET", raw] {
        assert!(!parsed.label().contains(secret));
        assert!(!debug.contains(secret));
    }
}

#[test]
fn uri_userinfo_and_query_passwords_do_not_reach_the_label() {
    let userinfo = target("postgresql://monitor:URI_SECRET@db.example:6543/DB_SECRET");
    assert_eq!(userinfo.label(), "monitor@db.example:6543");

    let raw = "postgresql://monitor@db.example/prod?password=QUERY_SECRET&options=OPTIONS_SECRET&application_name=APP_SECRET";
    let query = target(raw);
    assert_eq!(query.label(), "monitor@db.example:5432");
    let debug = format!("{query:?}");
    for secret in [
        "URI_SECRET",
        "QUERY_SECRET",
        "OPTIONS_SECRET",
        "APP_SECRET",
        "prod",
        raw,
    ] {
        assert!(!userinfo.label().contains(secret));
        assert!(!query.label().contains(secret));
        assert!(!debug.contains(secret));
    }
}

#[test]
fn percent_encoded_user_and_host_are_operator_readable() {
    let parsed = target("postgresql://mon%20itor:PCT%2FSECRET@db%2Eexample:6432/prod");

    assert_eq!(parsed.label(), "mon itor@db.example:6432");
    assert!(!parsed.label().contains("PCT/SECRET"));
    assert!(!parsed.label().contains("prod"));
}

#[test]
fn multiple_hosts_use_default_broadcast_and_per_host_ports() {
    assert_eq!(
        target("host=one,two user=monitor").label(),
        "monitor@one:5432,monitor@two:5432"
    );
    assert_eq!(
        target("host=one,two port=6000 user=monitor").label(),
        "monitor@one:6000,monitor@two:6000"
    );
    assert_eq!(
        target("host=one,two,three port=6000,,6002 user=monitor").label(),
        "monitor@one:6000,monitor@two:5432,monitor@three:6002"
    );
    assert_eq!(
        target("postgresql://monitor@one:6000,two,three:6002/prod").label(),
        "monitor@one:6000,monitor@two:5432,monitor@three:6002"
    );
}

#[test]
fn ipv6_is_bracketed_and_logical_host_wins_over_hostaddr() {
    assert_eq!(
        target("host=2001:db8::1 port=6432").label(),
        "server-default@[2001:db8::1]:6432"
    );
    assert_eq!(
        target("postgresql://[2001:db8::1]:6432/prod").label(),
        "server-default@[2001:db8::1]:6432"
    );
    assert_eq!(
        target("host=fe80::1%eth0 port=6432").label(),
        "server-default@[fe80::1%eth0]:6432"
    );
    assert_eq!(
        target("host=logical.example hostaddr=192.0.2.10 port=6543").label(),
        "server-default@logical.example:6543"
    );
    assert_eq!(
        target("hostaddr=2001:db8::2 port=6544").label(),
        "server-default@[2001:db8::2]:6544"
    );
}

#[cfg(unix)]
#[test]
fn unix_sockets_are_distinct_and_can_be_percent_encoded() {
    assert_eq!(
        target("host=/var/run/postgresql port=6433 user=monitor").label(),
        "monitor@unix:/var/run/postgresql:6433"
    );
    assert_eq!(
        target("postgresql:///prod?host=%2Fvar%2Frun%2Fpostgresql&port=6434&user=monitor").label(),
        "monitor@unix:/var/run/postgresql:6434"
    );
}

#[test]
fn missing_user_is_labelled_as_the_server_default() {
    assert_eq!(
        target("host=db.example port=6543").label(),
        "server-default@db.example:6543"
    );
}

#[test]
fn server_reported_user_replaces_the_default_label() {
    let parsed = target("host=db.example port=6543");
    assert_eq!(
        parsed.label_for_user("postgres"),
        "postgres@db.example:6543"
    );
}

#[test]
fn malformed_and_structurally_invalid_inputs_return_opaque_errors() {
    let raw = "host='unterminated MALFORMED_SECRET";
    let error = ConnectionTarget::parse(raw, 0).expect_err("reject malformed connection");
    assert_eq!(error, InvalidConnection);
    let rendered = format!("{error:?}");
    assert!(!rendered.contains("MALFORMED_SECRET"));
    assert!(!rendered.contains(raw));

    for invalid in [
        "user=monitor",
        "host=one,two hostaddr=192.0.2.1",
        "host=one,two port=6000,6001,6002",
        "host=db.example passfile=PASSFILE_SECRET",
    ] {
        let error = ConnectionTarget::parse(invalid, 0).expect_err("reject invalid connection");
        assert_eq!(error, InvalidConnection);
        assert!(!format!("{error:?}").contains("PASSFILE_SECRET"));
    }
}

#[test]
fn log_facts_and_identity_are_separate_marked_queries() {
    let parsed = target("host=db.example user=monitor");

    assert_eq!(parsed.source_index(), 7);
    assert!(POSTGRES_LOG_FACTS_QUERY.contains("pg_current_logfile()"));
    assert!(!POSTGRES_LOG_FACTS_QUERY.contains("pg_control_system()"));
    assert!(POSTGRES_SYSTEM_IDENTIFIER_QUERY.contains("pg_control_system()"));
    for sql in [POSTGRES_LOG_FACTS_QUERY, POSTGRES_SYSTEM_IDENTIFIER_QUERY] {
        assert!(sql.contains(env!("CARGO_PKG_VERSION")), "{sql}");
        assert!(sql.contains("log_sources/settings.rs"), "{sql}");
    }
}
