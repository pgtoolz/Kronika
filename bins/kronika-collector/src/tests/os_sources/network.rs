use std::path::Path;

use super::collect_protocol_counters;
use crate::os_sources::{OsSources, ProcFs};

const FILES: [(&str, &str); 5] = [
    ("net/snmp", "Tcp: ActiveOpens\nTcp: 11\n"),
    ("net/netstat", "TcpExt: ListenDrops\nTcpExt: 22\n"),
    ("net/snmp6", "Ip6InReceives 33\nUdp6InDatagrams 34\n"),
    ("net/rpc/nfs", "rpc 44 2 3\n"),
    ("net/rpc/nfsd", "rc 55 6 7\nrpc 56 0\n"),
];

fn write_protocol_files(root: &Path) {
    std::fs::create_dir_all(root.join("net/rpc")).expect("network proc directories");
    for (name, content) in FILES {
        std::fs::write(root.join(name), content).expect("network proc file");
    }
}

#[test]
fn protocol_counters_share_the_network_scope_and_tick_timestamp() {
    let dir = tempfile::tempdir().expect("proc root");
    write_protocol_files(dir.path());
    let fs = ProcFs::new(dir.path().to_path_buf());
    let mut os = OsSources::default();

    collect_protocol_counters(&fs, 3, 41, &mut os);

    let snmp = os.snmp.expect("SNMP counters");
    let netstat = os.netstat.expect("extended TCP counters");
    let snmp6 = os.snmp6.expect("IPv6 counters");
    let nfs_client = os.nfs_client.expect("NFS client counters");
    let nfs_server = os.nfs_server.expect("NFS server counters");
    assert_eq!(snmp.tcp_active_opens, 11);
    assert_eq!(netstat.listen_drops, 22);
    assert_eq!(snmp6.ip6_in_receives, Some(33));
    assert_eq!(nfs_client.rpc_calls, 44);
    assert_eq!(nfs_server.reply_cache_hits, 55);
    for identity in [
        (snmp.ts.0, snmp.scope),
        (netstat.ts.0, netstat.scope),
        (snmp6.ts.0, snmp6.scope),
        (nfs_client.ts.0, nfs_client.scope),
        (nfs_server.ts.0, nfs_server.scope),
    ] {
        assert_eq!(identity, (41, 3));
    }
}

#[test]
fn missing_or_unreadable_protocol_files_preserve_existing_rows() {
    for unreadable in [false, true] {
        let dir = tempfile::tempdir().expect("proc root");
        write_protocol_files(dir.path());
        let fs = ProcFs::new(dir.path().to_path_buf());
        let mut os = OsSources::default();
        collect_protocol_counters(&fs, 3, 41, &mut os);
        let prior = (os.snmp, os.netstat, os.snmp6, os.nfs_client, os.nfs_server);
        for (name, _) in FILES {
            let path = dir.path().join(name);
            std::fs::remove_file(&path).expect("remove network proc file");
            if unreadable {
                std::fs::create_dir(path).expect("directory cannot be read as a proc file");
            }
        }

        collect_protocol_counters(&fs, 0, 42, &mut os);

        assert_eq!(
            (os.snmp, os.netstat, os.snmp6, os.nfs_client, os.nfs_server),
            prior,
            "unreadable: {unreadable}"
        );
    }
}

#[test]
fn parse_errors_preserve_rows_while_absent_counters_replace_them() {
    let dir = tempfile::tempdir().expect("proc root");
    write_protocol_files(dir.path());
    let fs = ProcFs::new(dir.path().to_path_buf());
    let mut os = OsSources::default();
    collect_protocol_counters(&fs, 3, 41, &mut os);
    let prior = (os.snmp, os.netstat);
    for (name, content) in [
        ("net/snmp", "Tcp: ActiveOpens\nTcp: invalid\n"),
        ("net/netstat", "TcpExt: ListenDrops\nTcpExt: invalid\n"),
        ("net/snmp6", ""),
        ("net/rpc/nfs", "proc3 0\n"),
        ("net/rpc/nfsd", "rpc 9 0\n"),
    ] {
        std::fs::write(dir.path().join(name), content).expect("replace network proc file");
    }

    collect_protocol_counters(&fs, 0, 42, &mut os);

    assert_eq!((os.snmp, os.netstat), prior);
    assert_eq!(os.nfs_client, None);
    assert_eq!(os.nfs_server, None);
    let snmp6 = os.snmp6.expect("empty IPv6 file still produces a row");
    assert_eq!((snmp6.ts.0, snmp6.scope), (42, 0));
    assert_eq!(snmp6.ip6_in_receives, None);
    assert_eq!(snmp6.udp6_in_datagrams, None);
}
