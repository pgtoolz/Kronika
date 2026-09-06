use super::{parse_client, parse_server};

// 22 NFSv3 procedures: index 6 is READ, 7 is WRITE, 21 is COMMIT.
const CLIENT: &str = "\
net 0 0 0 0
rpc 500 3 7
proc3 22 1 2 3 4 5 6 700 800 9 10 11 12 13 14 15 16 17 18 19 20 21 900
";

const SERVER: &str = "\
rc 10 20 30
fh 0 0 0 0 0
io 4096 8192
th 8 0 0.000 0.000
net 111 0 111 0
rpc 400 1 0 0 0
";

#[test]
fn client_reads_rpc_and_the_v3_operations() {
    let row = parse_client(CLIENT, 42, 1).expect("an nfs client file");
    assert_eq!(row.ts.0, 42);
    assert_eq!(row.scope, 1);
    assert_eq!(row.rpc_calls, 500);
    assert_eq!(row.rpc_retrans, 3);
    assert_eq!(row.rpc_auth_refresh, 7);
    assert_eq!(row.op_read, 700);
    assert_eq!(row.op_write, 800);
    assert_eq!(row.op_commit, 900);
}

#[test]
fn a_host_with_no_nfs_client_yields_no_row() {
    assert!(parse_client("net 0 0 0 0\n", 1, 0).is_none());
    assert!(parse_client("", 1, 0).is_none());
}

#[test]
fn a_client_without_a_procedure_vector_still_reports_rpc() {
    let row = parse_client("rpc 12 0 0\n", 1, 0).expect("rpc line is enough");
    assert_eq!(row.rpc_calls, 12);
    assert_eq!(row.op_read, 0);
    assert_eq!(row.op_write, 0);
}

#[test]
fn server_reads_the_reply_cache_io_and_rpc_lines() {
    let row = parse_server(SERVER, 7, 0).expect("an nfs server file");
    assert_eq!(row.ts.0, 7);
    assert_eq!(row.reply_cache_hits, 10);
    assert_eq!(row.reply_cache_misses, 20);
    assert_eq!(row.reply_cache_nocache, 30);
    assert_eq!(row.io_read_bytes, 4_096);
    assert_eq!(row.io_write_bytes, 8_192);
    assert_eq!(row.net_count, 111);
    assert_eq!(row.rpc_calls, 400);
    assert_eq!(row.rpc_bad_calls, 1);
}

#[test]
fn a_host_that_exports_nothing_yields_no_row() {
    assert!(parse_server("io 0 0\n", 1, 0).is_none());
}

#[test]
fn a_prefix_that_is_not_a_whole_key_does_not_match() {
    // "rpcinfo" must not be read as the "rpc" line.
    assert!(parse_client("rpcinfo 1 2 3\n", 1, 0).is_none());
}
