use super::{MAX_PASSWD_BYTES, MAX_PASSWD_ENTRIES, MAX_PASSWD_LINE_BYTES, PasswdSnapshot, parse};
use std::io::Write;

#[test]
fn parses_names_and_keeps_first_duplicate_uid() {
    let snapshot = parse(
            b"root:x:0:0:root:/root:/bin/sh\npostgres:x:26:26::/var/lib/postgresql:/bin/false\nalias:x:26:26::/:/bin/false\n",
        )
        .expect("parse");
    assert_eq!(snapshot.username(0), Some("root"));
    assert_eq!(snapshot.username(26), Some("postgres"));
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot.rejected_lines(), 0);
}

#[test]
fn malformed_and_overlong_lines_are_skipped() {
    let mut bytes = b"bad\nvalid:x:1000:1000::/:/bin/sh\n".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', MAX_PASSWD_LINE_BYTES + 1));
    bytes.push(b'\n');
    let snapshot = parse(&bytes).expect("parse");
    assert_eq!(snapshot.username(1_000), Some("valid"));
    assert_eq!(snapshot.rejected_lines(), 2);
}

#[test]
fn rejects_excessive_record_count() {
    let bytes = "u:x:1:1::/:/bin/sh\n".repeat(MAX_PASSWD_ENTRIES + 1);
    assert!(parse(bytes.as_bytes()).is_err());
}

#[test]
fn file_read_is_bounded() {
    let mut file = tempfile::NamedTempFile::new().expect("tempfile");
    file.write_all(&vec![b'x'; MAX_PASSWD_BYTES + 1])
        .expect("write");
    assert!(PasswdSnapshot::read(file.path()).is_err());
}
