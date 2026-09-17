//! Probe decoding, resolved identity, and reuse through the real wire protocol.

use std::io::Read as _;
use std::net::TcpListener;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::Pool;

use super::super::probe::SERVER_PROBE_SQL;
use super::super::{PgCollector, PgObservation, QueryOutcome};
use super::protocol::{ProtocolColumn, accept_startup, write_protocol_row};

fn columns(visible: &str) -> Vec<ProtocolColumn> {
    // Deliberately differ from SELECT order: decoding follows SQL aliases.
    [
        ("username", "resolved_monitor"),
        ("full_visibility", visible),
        ("datid", "16384"),
        ("server_version_num", "160013"),
        ("database", "resolved_database"),
        ("usesysid", "16385"),
    ]
    .into_iter()
    .map(|(name, value)| ProtocolColumn {
        name,
        oid: 25,
        value: Some(value.as_bytes().to_vec()),
    })
    .collect()
}

fn fixture(responses: Vec<Vec<ProtocolColumn>>) -> (PgCollector, JoinHandle<usize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind probe fixture");
    let port = listener.local_addr().expect("fixture address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept probe connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("bound fixture reads");
        accept_startup(&mut stream);
        let mut responses = responses.into_iter();
        let mut queries = 0;
        loop {
            let mut header = [0; 5];
            match stream.read_exact(&mut header) {
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => panic!("read probe query: {error}"),
                Ok(()) => {}
            }
            if header[0] == b'X' {
                break;
            }
            assert_eq!(header[0], b'Q');
            let length = u32::from_be_bytes(header[1..].try_into().expect("length bytes"));
            let mut body = vec![0; usize::try_from(length - 4).expect("query length")];
            stream.read_exact(&mut body).expect("query body");
            assert_eq!(body.strip_suffix(&[0]), Some(SERVER_PROBE_SQL.as_bytes()));
            write_protocol_row(
                &mut stream,
                &responses.next().expect("expected query"),
                false,
            );
            queries += 1;
        }
        assert!(responses.next().is_none(), "all expected probes were read");
        queries
    });
    let sources = PgCollector {
        server: Some(
            Pool::new(&format!(
                "host=127.0.0.1 port={port} user=configured dbname=configured sslmode=disable"
            ))
            .expect("fixture DSN"),
        ),
        ..PgCollector::default()
    };
    (sources, server)
}

#[tokio::test]
async fn probe_resolves_identity_and_reuses_the_generation_until_refresh() {
    let (mut sources, server) = fixture(vec![columns("f"), columns("t")]);
    let mut observations = Vec::new();
    let probe = sources
        .read_probe(false, &mut |event| observations.push(event))
        .await
        .expect("initial probe");
    assert_eq!(
        (probe.major, probe.datid, probe.usesysid),
        (16, 16_384, 16_385)
    );
    assert_eq!(
        (probe.database.as_str(), probe.user.as_str()),
        ("resolved_database", "resolved_monitor")
    );
    assert!(!probe.full_visibility);
    let cached = sources
        .read_probe(false, &mut |event| observations.push(event))
        .await
        .expect("cached probe");
    assert_eq!(cached.generation, probe.generation);
    assert!(!cached.full_visibility);
    let refreshed = sources
        .read_probe(true, &mut |event| observations.push(event))
        .await
        .expect("refreshed probe");
    assert_eq!(refreshed.generation, probe.generation);
    assert!(refreshed.full_visibility);
    assert_eq!(observations.len(), 2, "cache hits do not execute a probe");
    assert!(observations.iter().all(|event| matches!(event,
        PgObservation::Query(query) if query.outcome == QueryOutcome::Success
            && query.database == "resolved_database"
            && query.connection.starts_with("resolved_monitor@")
    )));
    sources.close_connections();
    tokio::task::yield_now().await;
    assert_eq!(server.join().expect("fixture exits"), 2);
}

#[tokio::test]
async fn malformed_probe_rows_emit_errors_without_populating_the_cache() {
    for (name, value, expected) in [
        (
            "server_version_num",
            Some("custom-pg"),
            "parse server_version_num",
        ),
        ("full_visibility", Some("true"), "for visibility"),
        ("database", None, "server probe omitted current_database"),
        ("missing", None, "server_version_num"),
    ] {
        let mut response = columns("t");
        if name == "missing" {
            response.retain(|column| column.name != "server_version_num");
        } else {
            response
                .iter_mut()
                .find(|column| column.name == name)
                .expect("column")
                .value = value.map(|value| value.as_bytes().to_vec());
        }
        let (mut sources, server) = fixture(vec![response]);
        let mut observations = Vec::new();
        assert!(
            sources
                .read_probe(false, &mut |event| observations.push(event))
                .await
                .is_none()
        );
        assert!(sources.probe.is_none());
        assert!(sources.server_database.is_none());
        assert!(
            matches!(observations.as_slice(), [PgObservation::Query(query)]
                if query.query_name == "server_probe" && query.outcome == QueryOutcome::Error
                    && query.error.as_deref().is_some_and(|error| error.contains(expected))
            ),
            "{name}: {observations:?}"
        );
        sources.close_connections();
        tokio::task::yield_now().await;
        assert_eq!(server.join().expect("fixture exits"), 1);
    }
}
