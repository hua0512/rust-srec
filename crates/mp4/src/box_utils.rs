use bytes::Bytes;

/// Parsed view over a single ISOBMFF box inside a parent byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoxView {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) size: usize,
    pub(crate) header_size: usize,
    pub(crate) fourcc: [u8; 4],
    pub(crate) body_start: usize,
    pub(crate) body_end: usize,
}

/// Read a box header: returns `(total_box_size, fourcc, header_size)`.
///
/// Handles 32-bit size, 64-bit extended size (`size == 1`),
/// and box-extends-to-EOF (`size == 0`).
pub(crate) fn read_box_header(data: &[u8]) -> Option<(usize, [u8; 4], usize)> {
    if data.len() < 8 {
        return None;
    }

    let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
    let fourcc: [u8; 4] = [data[4], data[5], data[6], data[7]];

    if size == 1 {
        if data.len() < 16 {
            return None;
        }
        let ext_size = u64::from_be_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        Some((ext_size as usize, fourcc, 16))
    } else if size == 0 {
        Some((data.len(), fourcc, 8))
    } else {
        Some((size as usize, fourcc, 8))
    }
}

/// Parse a single box located at `offset` within `[0..end)`.
pub(crate) fn box_at(data: &Bytes, offset: usize, end: usize) -> Option<BoxView> {
    if offset >= end {
        return None;
    }

    let remaining = &data[offset..end];
    let (size, fourcc, header_size) = read_box_header(remaining)?;

    if size < header_size || offset + size > end {
        return None;
    }

    let body_start = offset + header_size;
    let body_end = offset + size;
    Some(BoxView {
        start: offset,
        end: offset + size,
        size,
        header_size,
        fourcc,
        body_start,
        body_end,
    })
}

/// Find the first child box with the given FourCC inside `[start..end)`.
pub(crate) fn find_first_box(
    data: &Bytes,
    start: usize,
    end: usize,
    target: [u8; 4],
) -> Option<BoxView> {
    let mut offset = start;
    while offset < end {
        let parsed = box_at(data, offset, end)?;
        if parsed.fourcc == target {
            return Some(parsed);
        }

        offset = parsed.end;
    }

    None
}

/// Find the first child box payload for the given FourCC inside `[start..end)`.
pub(crate) fn find_first_box_payload(
    data: &Bytes,
    start: usize,
    end: usize,
    target: [u8; 4],
) -> Option<Bytes> {
    let parsed = find_first_box(data, start, end, target)?;
    Some(data.slice(parsed.body_start..parsed.body_end))
}
