//! `PostgreSQL` protocol fixtures for collector query tests.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use crate::pg_sources::probe::SERVER_PROBE_SQL;

pub(super) fn serve_two_handshakes(listener: TcpListener, release: mpsc::Receiver<()>) {
    let (mut first, _peer) = listener.accept().expect("accept the first connection");
    accept_startup(&mut first);
    let (mut second, _peer) = listener.accept().expect("accept the second connection");
    accept_startup(&mut second);
    drop(listener);
    release.recv().expect("the test releases the connections");
    drop((first, second, release));
}

pub(super) fn accept_startup(stream: &mut TcpStream) {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).expect("read startup length");
    if u32::from_be_bytes(len) == 8 {
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).expect("read SSLRequest");
        assert_eq!(
            u32::from_be_bytes(request),
            80_877_103,
            "PostgreSQL SSLRequest"
        );
        stream.write_all(b"N").expect("probe declines TLS");
        stream.flush().expect("flush TLS refusal");
        stream
            .read_exact(&mut len)
            .expect("read plaintext startup length");
    }
    let body_len = usize::try_from(u32::from_be_bytes(len).saturating_sub(4))
        .expect("the startup body length fits usize");
    let mut body = vec![0_u8; body_len];
    stream.read_exact(&mut body).expect("read startup body");
    stream
        .write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'])
        .expect("write authentication and ready messages");
    stream.flush().expect("flush startup response");

    let mut tag = [0_u8; 1];
    stream.read_exact(&mut tag).expect("read setup tag");
    assert_eq!(tag[0], b'Q');
    stream.read_exact(&mut len).expect("read setup length");
    let body_len = usize::try_from(u32::from_be_bytes(len).saturating_sub(4))
        .expect("the setup body length fits usize");
    let mut body = vec![0_u8; body_len];
    stream.read_exact(&mut body).expect("read setup body");
    let sql = body.strip_suffix(&[0]).expect("setup SQL is terminated");
    let sql = std::str::from_utf8(sql).expect("setup SQL is UTF-8");
    assert!(sql.contains("SET statement_timeout = '30s'"));
    assert!(sql.contains("SET lock_timeout = '100ms'"));
    stream
        .write_all(&[
            b'C', 0, 0, 0, 8, b'S', b'E', b'T', 0, b'C', 0, 0, 0, 8, b'S', b'E', b'T', 0, b'Z', 0,
            0, 0, 5, b'I',
        ])
        .expect("write setup completion and ready messages");
    stream.flush().expect("flush setup response");
}

pub(super) fn serve_enumeration_denied(listener: &TcpListener) {
    let (mut stream, _) = listener.accept().expect("accept monitoring connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bound fixture reads");
    accept_startup(&mut stream);
    let mut sql = String::new();
    loop {
        let mut tag = [0];
        match stream.read_exact(&mut tag) {
            // Tests close the pool after collection. Cancelling the driver can
            // reset TCP when a ReadyForQuery reply remains unread at this boundary.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                break;
            }
            Err(error) => panic!("read frontend after response to {sql:?}: {error}"),
            Ok(()) => {}
        }
        let mut length = [0; 4];
        stream.read_exact(&mut length).expect("frontend length");
        let mut body =
            vec![0; usize::try_from(u32::from_be_bytes(length) - 4).expect("body length")];
        stream.read_exact(&mut body).expect("frontend body");
        match tag[0] {
            b'X' => break,
            b'Q' => {
                let query = std::str::from_utf8(body.strip_suffix(&[0]).expect("terminated SQL"))
                    .expect("simple SQL");
                if query == SERVER_PROBE_SQL {
                    let columns = [
                        "server_version_num",
                        "datid",
                        "database",
                        "usesysid",
                        "username",
                        "full_visibility",
                    ]
                    .into_iter()
                    .zip(["160000", "1", "metrics", "10", "monitor", "t"])
                    .map(|(name, value)| ProtocolColumn {
                        name,
                        oid: 25,
                        value: Some(value.as_bytes().to_vec()),
                    })
                    .collect::<Vec<_>>();
                    write_protocol_row(&mut stream, &columns, false);
                } else {
                    protocol_denied_source(&mut stream);
                }
            }
            b'P' => {
                let bytes = body.get(1..).expect("unnamed statement");
                sql = std::str::from_utf8(bytes.split(|byte| *byte == 0).next().expect("SQL"))
                    .expect("typed SQL")
                    .to_owned();
            }
            b'B' | b'D' | b'E' => {}
            b'S' => {
                if sql
                    == kronika_source_pg::activity::activity_query(
                        kronika_source_pg::activity::ActivityVersion::V3,
                    )
                    || sql
                        == kronika_source_pg::locks::locks_query(
                            kronika_source_pg::locks::LocksVersion::V2,
                        )
                {
                    protocol_backend(&mut stream, b'1', &[]);
                    protocol_backend(&mut stream, b'2', &[]);
                    write_protocol_row(&mut stream, &monitoring_protocol_columns(), true);
                } else {
                    protocol_denied_source(&mut stream);
                }
            }
            other => panic!("unexpected frontend tag {other}"),
        }
    }
}

pub(super) struct ProtocolColumn {
    pub(super) name: &'static str,
    pub(super) oid: u32,
    pub(super) value: Option<Vec<u8>>,
}

fn monitoring_protocol_columns() -> Vec<ProtocolColumn> {
    let mut columns = Vec::new();
    for (name, value) in [
        ("ts_us", Some(1_000_000_i64)),
        ("backend_start_us", Some(1)),
        ("query_id", None),
        ("backend_xid_age", None),
        ("backend_xmin_age", None),
        ("xact_start_us", None),
        ("query_start_us", None),
        ("state_change_us", None),
        ("lock_transactionid", None),
        ("waitstart_us", None),
    ] {
        columns.push(ProtocolColumn {
            name,
            oid: 20,
            value: value.map(|v| v.to_be_bytes().to_vec()),
        });
    }
    for (name, value) in [
        ("pid", Some(42_i32)),
        ("leader_pid", None),
        ("lock_page", None),
    ] {
        columns.push(ProtocolColumn {
            name,
            oid: 23,
            value: value.map(|v| v.to_be_bytes().to_vec()),
        });
    }
    for (name, value) in [
        ("datid", Some(1_u32)),
        ("lock_database", None),
        ("lock_relation", None),
        ("lock_classid", None),
        ("lock_objid", None),
    ] {
        columns.push(ProtocolColumn {
            name,
            oid: 26,
            value: value.map(|v| v.to_be_bytes().to_vec()),
        });
    }
    for name in ["lock_tuple", "lock_objsubid"] {
        columns.push(ProtocolColumn {
            name,
            oid: 21,
            value: None,
        });
    }
    for (name, value) in [
        ("datname", Some("metrics")),
        ("usename", Some("monitor")),
        ("application_name", Some("fixture")),
        ("client_addr", Some("127.0.0.1")),
        ("backend_type", Some("client backend")),
        ("state", Some("active")),
        ("query", Some("SELECT 1")),
        ("wait_event_type", None),
        ("wait_event", None),
        ("lock_locktype", None),
        ("lock_mode", None),
        ("lock_relname", None),
        ("lock_virtualxid", None),
        ("lock_target", None),
    ] {
        columns.push(ProtocolColumn {
            name,
            oid: 25,
            value: value.map(|v| v.as_bytes().to_vec()),
        });
    }
    columns.push(ProtocolColumn {
        name: "blocked_by",
        oid: 1007,
        value: Some(
            [
                0_i32.to_be_bytes(),
                0_i32.to_be_bytes(),
                23_i32.to_be_bytes(),
            ]
            .concat(),
        ),
    });
    columns
}

pub(super) fn write_protocol_row(stream: &mut TcpStream, columns: &[ProtocolColumn], binary: bool) {
    let count = i16::try_from(columns.len()).expect("small fixture row");
    let mut description = count.to_be_bytes().to_vec();
    let mut data = count.to_be_bytes().to_vec();
    for column in columns {
        description.extend_from_slice(column.name.as_bytes());
        description.push(0);
        description.extend_from_slice(&0_u32.to_be_bytes());
        description.extend_from_slice(&0_i16.to_be_bytes());
        description.extend_from_slice(&column.oid.to_be_bytes());
        description.extend_from_slice(&(-1_i16).to_be_bytes());
        description.extend_from_slice(&(-1_i32).to_be_bytes());
        description.extend_from_slice(&i16::from(binary).to_be_bytes());
        if let Some(value) = &column.value {
            data.extend_from_slice(
                &i32::try_from(value.len())
                    .expect("small value")
                    .to_be_bytes(),
            );
            data.extend_from_slice(value);
        } else {
            data.extend_from_slice(&(-1_i32).to_be_bytes());
        }
    }
    protocol_backend(stream, b'T', &description);
    protocol_backend(stream, b'D', &data);
    protocol_backend(stream, b'C', b"SELECT 1\0");
    protocol_backend(stream, b'Z', b"I");
    stream.flush().expect("flush row");
}

fn protocol_backend(stream: &mut TcpStream, tag: u8, body: &[u8]) {
    stream.write_all(&[tag]).expect("backend tag");
    stream
        .write_all(
            &u32::try_from(body.len() + 4)
                .expect("small message")
                .to_be_bytes(),
        )
        .expect("backend length");
    stream.write_all(body).expect("backend body");
}

fn protocol_denied_source(stream: &mut TcpStream) {
    protocol_backend(
        stream,
        b'E',
        b"SERROR\0C42501\0Mfixture denied independent source\0\0",
    );
    protocol_backend(stream, b'Z', b"I");
    stream.flush().expect("flush denied source");
}
