//! Types `1_119_001` and `1_120_001`: NFS client and server activity from
//! `/proc/net/rpc/nfs` and `/proc/net/rpc/nfsd`.

use crate::{Section, Ts};

/// NFS client RPC and operation counters.
///
/// A database on NFS storage stalls in the RPC layer long before the block
/// layer notices, so retransmissions and authentication refreshes are the
/// early signal. Absent when the host mounts no NFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_119_001,
    name = "os_nfs_client",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsNfsClient {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// RPC calls issued.
    #[column(c, unit = count)]
    pub rpc_calls: i64,
    /// RPC calls retransmitted.
    #[column(c, unit = count)]
    pub rpc_retrans: i64,
    /// Credential refreshes forced by an authentication failure.
    #[column(c, unit = count)]
    pub rpc_auth_refresh: i64,
    /// NFS `READ` operations.
    #[column(c, unit = count)]
    pub op_read: i64,
    /// NFS `WRITE` operations.
    #[column(c, unit = count)]
    pub op_write: i64,
    /// NFS `COMMIT` operations.
    #[column(c, unit = count)]
    pub op_commit: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// NFS server RPC, reply-cache, and I/O counters.
///
/// Only present when this host exports NFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_120_001,
    name = "os_nfs_server",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsNfsServer {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// RPC calls served.
    #[column(c, unit = count)]
    pub rpc_calls: i64,
    /// RPC calls rejected as malformed.
    #[column(c, unit = count)]
    pub rpc_bad_calls: i64,
    /// Reply-cache hits.
    #[column(c, unit = count)]
    pub reply_cache_hits: i64,
    /// Reply-cache misses.
    #[column(c, unit = count)]
    pub reply_cache_misses: i64,
    /// Requests that were not cacheable.
    #[column(c, unit = count)]
    pub reply_cache_nocache: i64,
    /// Bytes read out of exported filesystems.
    #[column(c, unit = bytes)]
    pub io_read_bytes: i64,
    /// Bytes written into exported filesystems.
    #[column(c, unit = bytes)]
    pub io_write_bytes: i64,
    /// Packets received on the server's transports.
    #[column(c, unit = count)]
    pub net_count: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
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
}
