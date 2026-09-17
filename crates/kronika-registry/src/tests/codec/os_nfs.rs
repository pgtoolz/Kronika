use super::{OsNfsClient, OsNfsServer};
use crate::{Section, Ts, contract::lint};

#[test]
fn contracts_pass_the_linter() {
    assert_eq!(lint(&[OsNfsClient::CONTRACT]), Ok(()));
    assert_eq!(lint(&[OsNfsServer::CONTRACT]), Ok(()));
}

#[test]
fn client_roundtrip() {
    crate::assert_roundtrips(&[OsNfsClient {
        ts: Ts(1),
        rpc_calls: 100,
        rpc_retrans: 2,
        rpc_auth_refresh: 3,
        op_read: 40,
        op_write: 50,
        op_commit: 6,
        scope: 0,
    }]);
}

#[test]
fn server_roundtrip() {
    crate::assert_roundtrips(&[OsNfsServer {
        ts: Ts(1),
        rpc_calls: 100,
        rpc_bad_calls: 0,
        reply_cache_hits: 10,
        reply_cache_misses: 20,
        reply_cache_nocache: 30,
        io_read_bytes: 4_096,
        io_write_bytes: 8_192,
        net_count: 111,
        scope: 0,
    }]);
}
