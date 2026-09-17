use super::ReadAt;
#[test]
fn slice_reads_at_offset_and_reports_len() {
    let data: &[u8] = b"0123456789";
    assert_eq!(ReadAt::byte_len(&data).unwrap(), 10);
    let mut buf = [0_u8; 3];
    data.read_exact_at(&mut buf, 4).unwrap();
    assert_eq!(&buf, b"456");
}
#[test]
fn slice_read_past_end_errors() {
    let data: &[u8] = b"abc";
    let mut buf = [0_u8; 4];
    assert!(data.read_exact_at(&mut buf, 0).is_err());
    assert!(data.read_exact_at(&mut buf, 3).is_err());
}
#[cfg(unix)]
#[test]
fn file_reads_at_offset() {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(b"hello world").unwrap();
    let file = std::fs::File::open(f.path()).unwrap();
    assert_eq!(ReadAt::byte_len(&file).unwrap(), 11);
    let mut buf = [0_u8; 5];
    file.read_exact_at(&mut buf, 6).unwrap();
    assert_eq!(&buf, b"world");
}
#[test]
fn vec_reads_at_offset_and_reports_len() {
    let data: Vec<u8> = b"0123456789".to_vec();
    assert_eq!(data.byte_len().unwrap(), 10);
    let mut buf = [0_u8; 3];
    data.read_exact_at(&mut buf, 4).unwrap();
    assert_eq!(&buf, b"456");
}
#[test]
fn vec_read_past_end_errors() {
    let data: Vec<u8> = b"abc".to_vec();
    let mut buf = [0_u8; 4];
    assert!(data.read_exact_at(&mut buf, 0).is_err());
    assert!(data.read_exact_at(&mut buf, 3).is_err());
}
