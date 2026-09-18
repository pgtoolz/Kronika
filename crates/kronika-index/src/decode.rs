use crate::IndexError;

fn bytes_at<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], IndexError> {
    bytes
        .get(at..at.checked_add(N).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)
}

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Result<u16, IndexError> {
    bytes_at(bytes, at).map(u16::from_le_bytes)
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Result<u32, IndexError> {
    bytes_at(bytes, at).map(u32::from_le_bytes)
}

pub(crate) fn u64_at(bytes: &[u8], at: usize) -> Result<u64, IndexError> {
    bytes_at(bytes, at).map(u64::from_le_bytes)
}

pub(crate) fn i64_at(bytes: &[u8], at: usize) -> Result<i64, IndexError> {
    bytes_at(bytes, at).map(i64::from_le_bytes)
}
