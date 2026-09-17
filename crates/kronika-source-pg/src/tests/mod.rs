#[allow(
    clippy::unnecessary_wraps,
    reason = "matches the fallible interner signature used by row converters"
)]
pub(crate) fn intern(bytes: &[u8]) -> Result<kronika_registry::StrId, std::convert::Infallible> {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    Ok(kronika_registry::StrId(hash | 1))
}
