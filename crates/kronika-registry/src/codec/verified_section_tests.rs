use bytes::Bytes;

use super::{CodecError, VerifiedSection};

#[test]
fn verify_accepts_a_matching_crc_and_rejects_a_mismatch() {
    let bytes = Bytes::from_static(b"section"); // len 7, the stand-in crc
    let crc = |b: &[u8]| u32::try_from(b.len()).unwrap_or(u32::MAX);
    assert!(VerifiedSection::verify(bytes.clone(), 7, crc).is_ok());
    assert!(matches!(
        VerifiedSection::verify(bytes, 99, crc),
        Err(CodecError::SectionCrcMismatch {
            expected: 99,
            got: 7
        })
    ));
}
