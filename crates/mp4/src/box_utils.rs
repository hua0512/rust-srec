use bytes::Bytes;

use crate::merge::MergeError;

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
    /// The header declared size 0: the box extends to the end of the file
    /// (ISO/IEC 14496-12 §4.2), which is only valid for a top-level box.
    pub(crate) open_ended: bool,
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
        // A size beyond the address space cannot describe bytes in memory.
        Some((usize::try_from(ext_size).ok()?, fourcc, 16))
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

    let remaining = data.get(offset..end)?;
    let (size, fourcc, header_size) = read_box_header(remaining)?;
    // A `uuid` box header carries a 16-byte extended type (§4.2).
    let header_size = if fourcc == *b"uuid" {
        header_size + 16
    } else {
        header_size
    };

    if size < header_size || offset.checked_add(size)? > end {
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
        open_ended: remaining[..4] == [0; 4],
    })
}

/// Strict iteration over the child boxes in `[start..end)` of `parent`.
///
/// Unlike [`box_at`], a truncated or oversized child is an error rather than
/// the end of iteration, and a size-0 child is rejected because "extends to
/// end of file" has no meaning inside a container.
pub(crate) fn children<'a>(
    data: &'a Bytes,
    start: usize,
    end: usize,
    parent: &'static [u8; 4],
) -> impl Iterator<Item = Result<BoxView, MergeError>> + 'a {
    let mut offset = start;
    std::iter::from_fn(move || {
        if offset >= end {
            return None;
        }
        let Some(child) = box_at(data, offset, end) else {
            offset = end;
            return Some(Err(MergeError::malformed(
                parent,
                "child box is truncated or larger than its parent",
            )));
        };
        if child.open_ended {
            offset = end;
            return Some(Err(MergeError::malformed(
                parent,
                "child box declares size 0 (extends to end of file)",
            )));
        }
        offset = child.end;
        Some(Ok(child))
    })
}

/// Strict iteration over the top-level boxes of a file or segment.
///
/// A size-0 box is accepted here; [`box_at`] already extends it to the end of
/// `data`, so it is necessarily the last box.
pub(crate) fn top_level_boxes(
    data: &Bytes,
) -> impl Iterator<Item = Result<BoxView, MergeError>> + '_ {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if offset >= data.len() {
            return None;
        }
        let Some(item) = box_at(data, offset, data.len()) else {
            offset = data.len();
            return Some(Err(MergeError::Malformed {
                fourcc: crate::FourCc(*b"file"),
                reason: "top-level box is truncated",
            }));
        };
        offset = item.end;
        Some(Ok(item))
    })
}

/// Big-endian reader over one box body that reports truncation as
/// [`MergeError::Malformed`] for that box.
pub(crate) struct BodyReader<'a> {
    data: &'a [u8],
    pos: usize,
    fourcc: [u8; 4],
}

impl<'a> BodyReader<'a> {
    pub(crate) fn new(data: &'a [u8], fourcc: [u8; 4]) -> Self {
        Self {
            data,
            pos: 0,
            fourcc,
        }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], MergeError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| MergeError::malformed(&self.fourcc, "box body is truncated"))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], MergeError> {
        let bytes = self.take(N)?;
        let mut out = [0; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, MergeError> {
        self.array().map(u32::from_be_bytes)
    }

    pub(crate) fn i32(&mut self) -> Result<i32, MergeError> {
        self.array().map(i32::from_be_bytes)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, MergeError> {
        self.array().map(u64::from_be_bytes)
    }

    /// FullBox header (§4.2.2): 8-bit version and 24-bit flags.
    pub(crate) fn full_box_header(&mut self) -> Result<(u8, u32), MergeError> {
        let word = self.u32()?;
        Ok(((word >> 24) as u8, word & 0x00ff_ffff))
    }

    /// Fails if any bytes are left, so that a body longer than its declared
    /// fields describe is never silently accepted.
    pub(crate) fn finish(&self) -> Result<(), MergeError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MergeError::malformed(
                &self.fourcc,
                "unexpected trailing bytes in box body",
            ))
        }
    }
}

/// Write a complete box: header, then whatever `body` appends.
///
/// The size is patched after the body is written. Boxes produced through this
/// function (headers and initialization data) never approach 4 GiB, so a
/// larger body is reported as an overflow instead of switching to a 64-bit
/// size; media data uses [`write_box_header`] instead.
pub(crate) fn write_box(
    out: &mut Vec<u8>,
    fourcc: &[u8; 4],
    body: impl FnOnce(&mut Vec<u8>) -> Result<(), MergeError>,
) -> Result<(), MergeError> {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(fourcc);
    body(out)?;
    let size = u32::try_from(out.len() - start).map_err(|_| MergeError::Overflow("box size"))?;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
    Ok(())
}

/// FullBox header (§4.2.2): 8-bit version and 24-bit flags.
pub(crate) fn write_full_box_header(out: &mut Vec<u8>, version: u8, flags: u32) {
    debug_assert!(flags <= 0x00ff_ffff, "FullBox flags are 24 bits");
    out.extend_from_slice(&((u32::from(version) << 24) | (flags & 0x00ff_ffff)).to_be_bytes());
}

/// Write a box header for a body of `body_len` bytes, using the 64-bit
/// `largesize` form only when the 32-bit size cannot represent it (§4.2).
pub(crate) fn write_box_header(out: &mut Vec<u8>, fourcc: &[u8; 4], body_len: u64) {
    match u32::try_from(body_len + 8) {
        Ok(size) => {
            out.extend_from_slice(&size.to_be_bytes());
            out.extend_from_slice(fourcc);
        }
        Err(_) => {
            out.extend_from_slice(&1u32.to_be_bytes());
            out.extend_from_slice(fourcc);
            out.extend_from_slice(&(body_len + 16).to_be_bytes());
        }
    }
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
