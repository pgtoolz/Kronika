//! Parse `/proc/net/rpc/nfs` (`1_119`) and `/proc/net/rpc/nfsd` (`1_120`).

use kronika_registry::Ts;
use kronika_registry::os_nfs::{OsNfsClient, OsNfsServer};

/// Values of one `key n v1 v2 ...` line, where the first token after the key
/// is a count of the values that follow.
fn counted_line<'a>(content: &'a str, key: &str) -> Option<Vec<&'a str>> {
    let rest = content
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))?;
    Some(rest.split_whitespace().collect())
}

/// Field `index` of a counted line, or `0` when the kernel prints a shorter
/// line than this build expects.
fn at(values: Option<&Vec<&str>>, index: usize) -> i64 {
    values
        .and_then(|v| v.get(index))
        .and_then(|token| token.parse().ok())
        .unwrap_or(0)
}

/// Build the `1_119_001` row from `/proc/net/rpc/nfs`.
///
/// `rpc` is a flat line; `proc3`/`proc4` are per-operation vectors whose first
/// token is the vector length. Only the operations an operator watches on a
/// database host are extracted. Returns `None` when the file has no `rpc`
/// line, which is what an unmounted client looks like.
#[must_use]
pub fn parse_client(content: &str, ts: i64, scope: u8) -> Option<OsNfsClient> {
    let rpc = counted_line(content, "rpc")?;
    let proc3 = counted_line(content, "proc3");
    let proc4 = counted_line(content, "proc4");
    // NFSv3 procedure numbers, offset by one for the leading vector length.
    let v3 = |op: usize| at(proc3.as_ref(), op + 1);
    // NFSv4 client operations use a different vector; sum both so a v4-only
    // mount is not silently reported as idle.
    let v4 = |op: usize| at(proc4.as_ref(), op + 1);
    Some(OsNfsClient {
        ts: Ts(ts),
        rpc_calls: at(Some(&rpc), 0),
        rpc_retrans: at(Some(&rpc), 1),
        rpc_auth_refresh: at(Some(&rpc), 2),
        op_read: v3(6) + v4(1),
        op_write: v3(7) + v4(2),
        op_commit: v3(21) + v4(5),
        scope,
    })
}

/// Build the `1_120_001` row from `/proc/net/rpc/nfsd`.
///
/// Returns `None` when the file has no `rc` line, which is what a host that
/// exports nothing looks like.
#[must_use]
pub fn parse_server(content: &str, ts: i64, scope: u8) -> Option<OsNfsServer> {
    let reply_cache = counted_line(content, "rc")?;
    let io = counted_line(content, "io");
    let net = counted_line(content, "net");
    let rpc = counted_line(content, "rpc");
    Some(OsNfsServer {
        ts: Ts(ts),
        rpc_calls: at(rpc.as_ref(), 0),
        rpc_bad_calls: at(rpc.as_ref(), 1),
        reply_cache_hits: at(Some(&reply_cache), 0),
        reply_cache_misses: at(Some(&reply_cache), 1),
        reply_cache_nocache: at(Some(&reply_cache), 2),
        io_read_bytes: at(io.as_ref(), 0),
        io_write_bytes: at(io.as_ref(), 1),
        net_count: at(net.as_ref(), 0),
        scope,
    })
}

#[cfg(test)]
mod tests;
