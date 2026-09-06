use crate::IndexError;

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Result<u16, IndexError> {
    let raw: [u8; 2] = bytes
        .get(at..at.checked_add(2).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(u16::from_le_bytes(raw))
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Result<u32, IndexError> {
    let raw: [u8; 4] = bytes
        .get(at..at.checked_add(4).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(u32::from_le_bytes(raw))
}

pub(crate) fn u64_at(bytes: &[u8], at: usize) -> Result<u64, IndexError> {
    let raw: [u8; 8] = bytes
        .get(at..at.checked_add(8).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(u64::from_le_bytes(raw))
}

pub(crate) fn i64_at(bytes: &[u8], at: usize) -> Result<i64, IndexError> {
    let raw: [u8; 8] = bytes
        .get(at..at.checked_add(8).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(i64::from_le_bytes(raw))
}
