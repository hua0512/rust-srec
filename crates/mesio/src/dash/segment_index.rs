//! Reading the Segment Index of `SegmentBase` Representations.
//!
//! A `SegmentBase` Representation is one resource; its subsegments are only
//! known from the Segment Index box (`sidx`, ISO/IEC 14496-12 §8.16.3) at
//! `@indexRange`. When a static MPD is fetched, the index of every
//! Representation that a recorded track selects, in every Period, is read
//! and applied to the manifest before any planner sees it. A `sidx` may
//! point to further `sidx` boxes (hierarchical or daisy-chained indexes);
//! they are followed in presentation order.
//!
//! Limits apply per Representation (`MAX_SUBSEGMENTS`,
//! [`MAX_SEGMENT_INDEX_BOXES`], [`MAX_SEGMENT_INDEX_BYTES`]) and across the
//! MPD ([`MAX_MPD_SUBSEGMENTS`], [`MAX_MPD_SEGMENT_INDEX_BOXES`],
//! [`MAX_MPD_SEGMENT_INDEX_BYTES`]); a resource indexed from several Periods
//! is read once.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use mp4::sidx::{BoxExtent, ReferenceKind, SegmentIndex, SegmentReference, locate};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use url::Url;

use crate::dash::config::{DashConfig, DashSelectionConfig};
use crate::dash::manifest::limits::{
    MAX_MPD_SEGMENT_INDEX_BOXES, MAX_MPD_SEGMENT_INDEX_BYTES, MAX_MPD_SUBSEGMENTS,
    MAX_SEGMENT_INDEX_BOXES, MAX_SEGMENT_INDEX_BYTES, MAX_SUBSEGMENTS,
};
use crate::dash::manifest::{
    ByteRange, Manifest, ManifestError, Representation, SegmentIndexLocation, Subsegment,
    SubsegmentIndex,
};
use crate::dash::selection;
use crate::downloader::ClientPool;
use crate::error::{DownloadError, SharedError};
use crate::segment_engine::descriptor::{MediaTrack, inherit_query_params};
use crate::segment_engine::fetch::{RangeRequest, fetch_range};
use crate::segment_engine::identity::ByteRangeKey;

/// Bytes read first at a nested `sidx`, whose size is not known before its
/// header is read; enough for one with 300 references.
const NESTED_READ_BYTES: u64 = 4096;

/// Segment Indexes read at the same time when a presentation has many
/// Periods.
const CONCURRENT_READS: usize = 4;

/// Reads byte ranges of a resource.
pub(crate) trait RangeReader {
    fn read(
        &self,
        url: &Url,
        range: ByteRange,
    ) -> impl Future<Output = Result<Bytes, DownloadError>> + Send;
}

/// Reads ranges over HTTP with the MPD request settings: its timeout, and
/// its retry budget for network errors and server errors.
pub(crate) struct HttpRangeReader<'a> {
    pub clients: &'a ClientPool,
    pub config: &'a DashConfig,
    /// Query parameters of the MPD URL, inherited as by segment URLs.
    pub parent_params: &'a [(String, String)],
    pub cancel: &'a CancellationToken,
    /// Until when requests may wait out rate limiting (see
    /// `crate::rate_limit`); rate limiting does not spend the retry budget.
    pub throttle_deadline: Instant,
}

impl RangeReader for HttpRangeReader<'_> {
    async fn read(&self, url: &Url, range: ByteRange) -> Result<Bytes, DownloadError> {
        let mut url = url.clone();
        inherit_query_params(&mut url, self.parent_params);
        let key = ByteRangeKey {
            offset: range.first,
            length: range_len(range)?,
        };
        let manifest = &self.config.manifest;
        let mut failures: u32 = 0;
        loop {
            let error = match fetch_range(
                self.clients,
                &url,
                key,
                &RangeRequest {
                    params: &self.config.base.params,
                    timeout: manifest.fetch_timeout,
                    cancel: self.cancel,
                    operation: "Segment Index fetch",
                    throttle_deadline: self.throttle_deadline,
                },
            )
            .await
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => error,
            };
            // The next request waits out the cool-down itself, up to the
            // deadline (see `fetch_range`).
            if matches!(error, DownloadError::RateLimited { .. }) {
                if Instant::now() >= self.throttle_deadline {
                    return Err(error);
                }
                debug!(error = %error, "Segment Index request rate limited; retrying");
                continue;
            }
            failures += 1;
            if matches!(error, DownloadError::Cancelled)
                || error.is_non_recoverable_source_error()
                || failures > manifest.max_refresh_retries
            {
                return Err(error);
            }
            debug!(failures, error = %error, "Segment Index request failed; retrying");
            let delay = manifest.retry_delay.saturating_mul(failures);
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(DownloadError::Cancelled),
                _ = tokio::time::sleep(delay.min(Duration::from_secs(60))) => {}
            }
        }
    }
}

/// What an index that cannot be read or applied does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnIndexFailure {
    /// Fail: recording is about to start with exactly these
    /// Representations.
    Fail,
    /// Leave the Representation's index unread, with a warning and the
    /// failure recorded on it. A running planner keeps the Representation it
    /// records while that stays usable, so it may not need this one; if it
    /// does, it fails with the recorded failure.
    Skip,
}

/// Reads and applies the Segment Index of every `SegmentBase`
/// Representation that one of `tracks` selects, in every Period. Other
/// Representations keep their index unread.
pub(crate) async fn read_selected(
    manifest: &mut Manifest,
    selection: &DashSelectionConfig,
    tracks: &[MediaTrack],
    reader: &impl RangeReader,
    on_failure: OnIndexFailure,
) -> Result<(), DownloadError> {
    // Representations to update, grouped by the index they share.
    let mut users: HashMap<SegmentIndexLocation, Vec<(usize, String, MediaTrack)>> = HashMap::new();
    let mut locations = Vec::new();
    for (period_index, period) in manifest.periods.iter().enumerate() {
        for &track in tracks {
            let Some(selected) = selection::select(period, track, selection) else {
                continue;
            };
            let Some(location) = selected.pending_segment_index() else {
                continue;
            };
            let entry = users.entry(location.clone()).or_insert_with(|| {
                locations.push(location.clone());
                Vec::new()
            });
            entry.push((period_index, selected.id.clone(), track));
        }
    }
    if locations.is_empty() {
        return Ok(());
    }
    info!(count = locations.len(), "reading DASH Segment Indexes");
    let budget = MpdBudget::default();
    let budget = &budget;
    let mut reads = futures::stream::iter(locations)
        .map(|location| async move {
            let index = read_segment_index(reader, &location, budget).await;
            (location, index)
        })
        .buffer_unordered(CONCURRENT_READS);
    // Applied as each completes, so only the reads in flight hold
    // unshared subsegment lists.
    while let Some((location, read)) = reads.next().await {
        let users = users.remove(&location).unwrap_or_default();
        let index = read.and_then(|read| {
            Ok(SubsegmentIndex::new(
                read.timescale,
                read.earliest_presentation_time,
                read.index_offset,
                &read.subsegments,
            )?)
        });
        let index = match index {
            Ok(index) => index,
            Err(error) => {
                if fails(on_failure, &error) {
                    return Err(error);
                }
                let error = SharedError::new(error);
                for (period, id, track) in users {
                    warn_unread(error.get(), track, &id);
                    if let Some(representation) = representation_mut(manifest, period, track, &id) {
                        representation.fail_segment_index(error.clone());
                    }
                }
                continue;
            }
        };
        for (period, id, track) in users {
            let Some(representation) = representation_mut(manifest, period, track, &id) else {
                continue;
            };
            match representation.apply_segment_index(&index) {
                Ok(()) => {
                    debug!(%track, representation = %id, subsegments = index.len(), "Segment Index read");
                }
                Err(error) => {
                    let error = DownloadError::from(error);
                    if fails(on_failure, &error) {
                        return Err(error);
                    }
                    warn_unread(&error, track, &id);
                    representation.fail_segment_index(SharedError::new(error));
                }
            }
        }
    }
    Ok(())
}

fn representation_mut<'a>(
    manifest: &'a mut Manifest,
    period: usize,
    track: MediaTrack,
    id: &str,
) -> Option<&'a mut Representation> {
    manifest
        .periods
        .get_mut(period)
        .and_then(|period| selection::find_mut(period, track, id))
}

/// Whether an index that could not be read or applied ends the read.
/// Cancellation always does.
fn fails(on_failure: OnIndexFailure, error: &DownloadError) -> bool {
    on_failure == OnIndexFailure::Fail || matches!(error, DownloadError::Cancelled)
}

fn warn_unread(error: &DownloadError, track: MediaTrack, representation: &str) {
    warn!(
        %track,
        representation,
        error = %crate::redact::redact_diagnostic(error),
        "Segment Index could not be read; the Representation stays unusable"
    );
}

/// Bytes in `range`; an MPD can write `0-18446744073709551615`.
fn range_len(range: ByteRange) -> Result<u64, ManifestError> {
    range
        .last
        .checked_sub(range.first)
        .and_then(|span| span.checked_add(1))
        .ok_or(ManifestError::overflow("byte range length"))
}

/// Index data read for one MPD, shared by its concurrent reads.
#[derive(Default)]
struct MpdBudget {
    boxes: AtomicU64,
    bytes: AtomicU64,
    subsegments: AtomicU64,
}

/// Adds `amount` to `counter`, failing once the total passes `limit`.
fn charge(
    counter: &AtomicU64,
    amount: u64,
    limit: usize,
    what: &'static str,
) -> Result<(), ManifestError> {
    let total = counter
        .fetch_add(amount, Ordering::Relaxed)
        .saturating_add(amount);
    if total > limit as u64 {
        return Err(ManifestError::LimitExceeded { what, limit });
    }
    Ok(())
}

/// What one Representation's index has read so far, checked against its own
/// limits and the MPD's.
struct Spent<'a> {
    mpd: &'a MpdBudget,
    boxes: AtomicU64,
    bytes: AtomicU64,
}

impl Spent<'_> {
    fn charge_bytes(&self, bytes: u64) -> Result<(), ManifestError> {
        charge(
            &self.bytes,
            bytes,
            MAX_SEGMENT_INDEX_BYTES,
            "Segment Index bytes per Representation",
        )?;
        charge(
            &self.mpd.bytes,
            bytes,
            MAX_MPD_SEGMENT_INDEX_BYTES,
            "Segment Index bytes per MPD",
        )
    }

    fn charge_box(&self) -> Result<(), ManifestError> {
        charge(
            &self.boxes,
            1,
            MAX_SEGMENT_INDEX_BOXES,
            "Segment Index boxes per Representation",
        )?;
        charge(
            &self.mpd.boxes,
            1,
            MAX_MPD_SEGMENT_INDEX_BOXES,
            "Segment Index boxes per MPD",
        )
    }

    fn charge_subsegment(&self, subsegments: usize) -> Result<(), ManifestError> {
        if subsegments > MAX_SUBSEGMENTS {
            return Err(ManifestError::LimitExceeded {
                what: "subsegments per Segment Index",
                limit: MAX_SUBSEGMENTS,
            });
        }
        charge(
            &self.mpd.subsegments,
            1,
            MAX_MPD_SUBSEGMENTS,
            "subsegments per MPD",
        )
    }
}

/// A decoded `sidx` box, the file offset where it starts and that of its
/// anchor point (the first byte after it), from which its references count.
struct ReadIndex {
    index: SegmentIndex,
    start: u64,
    anchor: u64,
}

impl ReadIndex {
    /// File offset of the first referenced item.
    fn first_item(&self) -> Result<u64, ManifestError> {
        self.anchor
            .checked_add(self.index.first_offset)
            .ok_or(ManifestError::overflow("Segment Index offset"))
    }
}

/// The media subsegments listed by a Segment Index, in presentation order.
#[derive(Debug)]
struct Subsegments {
    timescale: u64,
    earliest_presentation_time: u64,
    /// File offset of the top-level `sidx` box.
    index_offset: u64,
    subsegments: Vec<Subsegment>,
}

/// One level of the index being walked: the references not yet visited
/// and where the next one starts.
struct Level {
    references: std::vec::IntoIter<SegmentReference>,
    position: u64,
}

/// Reads the Segment Index at `location` and every `sidx` it points to,
/// listing the media subsegments in presentation order.
///
/// A reference's `referenced_size` runs to the next item of the same index,
/// so a reference to a nested `sidx` spans the media that follows that box
/// only when the media comes before the next nested box; the media of a
/// nested index is located from its own anchor point either way. Times come
/// from the top-level index's earliest presentation time and the media
/// references' durations; the durations of references to nested indexes are
/// not cross-checked. All boxes must index the same stream with the same
/// timescale.
async fn read_segment_index(
    reader: &impl RangeReader,
    location: &SegmentIndexLocation,
    mpd: &MpdBudget,
) -> Result<Subsegments, DownloadError> {
    let url = &location.url;
    let spent = Spent {
        mpd,
        boxes: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
    };
    let top = read_top(reader, url, location.range, &spent).await?;
    let (timescale, reference_id) = (top.index.timescale, top.index.reference_id);
    let earliest_presentation_time = top.index.earliest_presentation_time;
    let index_offset = top.start;
    let mut subsegments = Vec::new();
    let mut stack = vec![Level {
        position: top.first_item()?,
        references: top.index.references.into_iter(),
    }];
    while let Some(level) = stack.last_mut() {
        let Some(reference) = level.references.next() else {
            stack.pop();
            continue;
        };
        let size = u64::from(reference.referenced_size);
        if size == 0 {
            return Err(ManifestError::segment_index("a reference has no bytes").into());
        }
        let start = level.position;
        let end = start
            .checked_add(size)
            .ok_or(ManifestError::overflow("Segment Index offset"))?;
        level.position = end;
        match reference.kind {
            ReferenceKind::Media => {
                spent.charge_subsegment(subsegments.len() + 1)?;
                subsegments.push(Subsegment {
                    range: ByteRange {
                        first: start,
                        last: end - 1,
                    },
                    duration: u64::from(reference.subsegment_duration),
                });
            }
            ReferenceKind::Index => {
                let nested = read_nested(reader, url, start, size, &spent).await?;
                if nested.index.timescale != timescale || nested.index.reference_id != reference_id
                {
                    return Err(ManifestError::segment_index(
                        "a nested index describes another stream or timescale",
                    )
                    .into());
                }
                stack.push(Level {
                    position: nested.first_item()?,
                    references: nested.index.references.into_iter(),
                });
            }
        }
    }
    Ok(Subsegments {
        timescale: u64::from(timescale),
        earliest_presentation_time,
        index_offset,
        subsegments,
    })
}

/// Reads the top-level `sidx`: the first one in `@indexRange`, which may
/// extend past the range when the MPD rounded it down.
async fn read_top(
    reader: &impl RangeReader,
    url: &Url,
    range: ByteRange,
    spent: &Spent<'_>,
) -> Result<ReadIndex, DownloadError> {
    spent.charge_bytes(range_len(range)?)?;
    let data = reader.read(url, range).await?;
    let extent = locate(&data)
        .map_err(segment_index_error)?
        .ok_or_else(|| ManifestError::segment_index("@indexRange holds no Segment Index box"))?;
    finish_box(reader, url, range.first, data, extent, spent).await
}

/// Reads a `sidx` that a reference points to: it starts at `offset` and
/// fits in the reference's `size` bytes.
async fn read_nested(
    reader: &impl RangeReader,
    url: &Url,
    offset: u64,
    size: u64,
    spent: &Spent<'_>,
) -> Result<ReadIndex, DownloadError> {
    let first_read = size.min(NESTED_READ_BYTES);
    spent.charge_bytes(first_read)?;
    let range = span(offset, first_read)?;
    let data = reader.read(url, range).await?;
    let extent = locate(&data).map_err(segment_index_error)?;
    let Some(extent) = extent.filter(|extent| extent.start == 0) else {
        return Err(ManifestError::segment_index(
            "a reference to a Segment Index does not point to one",
        )
        .into());
    };
    if extent.end > size {
        return Err(ManifestError::segment_index(
            "a nested Segment Index is larger than the reference to it",
        )
        .into());
    }
    finish_box(reader, url, offset, data, extent, spent).await
}

/// The `len` bytes at `offset`, as an inclusive range; `len` is positive.
fn span(offset: u64, len: u64) -> Result<ByteRange, ManifestError> {
    let last = len
        .checked_sub(1)
        .and_then(|tail| offset.checked_add(tail))
        .ok_or(ManifestError::overflow("Segment Index offset"))?;
    Ok(ByteRange {
        first: offset,
        last,
    })
}

/// Completes and decodes the `sidx` at `extent` of `data`, which was read
/// from file offset `offset`, reading the rest of the box if needed.
async fn finish_box(
    reader: &impl RangeReader,
    url: &Url,
    offset: u64,
    data: Bytes,
    extent: BoxExtent,
    spent: &Spent<'_>,
) -> Result<ReadIndex, DownloadError> {
    spent.charge_box()?;
    // The anchor point is checked first, so every offset below it fits.
    let anchor = offset
        .checked_add(extent.end)
        .ok_or(ManifestError::overflow("Segment Index offset"))?;
    let have = data.len() as u64;
    let data = if extent.end > have {
        let missing = extent.end - have;
        spent.charge_bytes(missing)?;
        let rest = reader.read(url, span(offset + have, missing)?).await?;
        let mut whole = BytesMut::with_capacity(data.len() + rest.len());
        whole.extend_from_slice(&data);
        whole.extend_from_slice(&rest);
        whole.freeze()
    } else {
        data
    };
    let body = usize::try_from(extent.start)
        .ok()
        .zip(usize::try_from(extent.end).ok())
        .and_then(|(start, end)| data.get(start..end))
        .ok_or_else(|| ManifestError::segment_index("the Segment Index box is truncated"))?;
    let index = SegmentIndex::parse(body).map_err(segment_index_error)?;
    Ok(ReadIndex {
        index,
        // Within the anchor, which fits.
        start: offset + extent.start,
        anchor,
    })
}

fn segment_index_error(error: mp4::sidx::SidxError) -> ManifestError {
    ManifestError::segment_index(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::dash::config::VideoSelection;

    /// A reference: `(is_index, size, duration)`.
    type Reference = (bool, u32, u32);

    fn sidx(timescale: u32, ept: u64, first_offset: u64, references: &[Reference]) -> Vec<u8> {
        let mut body = vec![1, 0, 0, 0];
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&ept.to_be_bytes());
        body.extend_from_slice(&first_offset.to_be_bytes());
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(&(references.len() as u16).to_be_bytes());
        for &(index, size, duration) in references {
            body.extend_from_slice(&((u32::from(index) << 31) | size).to_be_bytes());
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&0x9000_0000u32.to_be_bytes());
        }
        let mut data = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        data.extend_from_slice(b"sidx");
        data.extend(body);
        data
    }

    fn sidx_len(references: usize) -> u32 {
        (8 + 32 + 12 * references) as u32
    }

    fn filler(fourcc: &[u8; 4], len: usize) -> Vec<u8> {
        let mut data = (len as u32).to_be_bytes().to_vec();
        data.extend_from_slice(fourcc);
        data.resize(len, 0xab);
        data
    }

    #[derive(Default)]
    struct MemoryReader {
        files: HashMap<String, Vec<u8>>,
        reads: Mutex<Vec<(String, u64, u64)>>,
    }

    impl MemoryReader {
        fn with(url: &str, data: Vec<u8>) -> Self {
            let mut reader = Self::default();
            reader.files.insert(url.to_owned(), data);
            reader
        }

        fn reads(&self) -> Vec<(String, u64, u64)> {
            self.reads.lock().unwrap().clone()
        }
    }

    impl RangeReader for MemoryReader {
        async fn read(&self, url: &Url, range: ByteRange) -> Result<Bytes, DownloadError> {
            self.reads
                .lock()
                .unwrap()
                .push((url.to_string(), range.first, range.last));
            let Some(data) = self.files.get(url.as_str()) else {
                return Err(DownloadError::http_status(
                    reqwest::StatusCode::GONE,
                    url.as_str(),
                    "Segment Index fetch",
                ));
            };
            let end = (range.last + 1).min(data.len() as u64);
            Ok(Bytes::copy_from_slice(
                &data[range.first as usize..end as usize],
            ))
        }
    }

    const URL: &str = "https://cdn.example.com/v.mp4";

    fn location(first: u64, last: u64) -> SegmentIndexLocation {
        SegmentIndexLocation {
            url: Url::parse(URL).unwrap(),
            range: ByteRange { first, last },
        }
    }

    fn ranges(index: &Subsegments) -> Vec<(u64, u64, u64)> {
        index
            .subsegments
            .iter()
            .map(|s| (s.range.first, s.range.last, s.duration))
            .collect()
    }

    async fn read(reader: &MemoryReader, location: &SegmentIndexLocation) -> Subsegments {
        read_segment_index(reader, location, &MpdBudget::default())
            .await
            .unwrap()
    }

    async fn error(reader: &MemoryReader, location: &SegmentIndexLocation) -> String {
        read_segment_index(reader, location, &MpdBudget::default())
            .await
            .unwrap_err()
            .to_string()
    }

    #[tokio::test]
    async fn flat_index_lists_subsegments_from_the_anchor() {
        let mut file = filler(b"ftyp", 24);
        file.extend(filler(b"moov", 100));
        let index_at = file.len() as u64;
        // A 16-byte gap between the index and the first subsegment.
        let index = sidx(1000, 40, 16, &[(false, 300, 2000), (false, 200, 1500)]);
        let anchor = index_at + index.len() as u64;
        file.extend(&index);
        file.resize(file.len() + 16 + 500, 0);
        let reader = MemoryReader::with(URL, file);
        let read = read(&reader, &location(index_at, anchor - 1)).await;
        assert_eq!(
            (read.timescale, read.earliest_presentation_time),
            (1000, 40)
        );
        let first = anchor + 16;
        assert_eq!(
            ranges(&read),
            [(first, first + 299, 2000), (first + 300, first + 499, 1500)]
        );
        assert_eq!(reader.reads().len(), 1);
    }

    #[tokio::test]
    async fn an_index_range_that_ends_early_is_extended_to_the_box() {
        let mut file = filler(b"styp", 16);
        let index = sidx(90_000, 0, 0, &[(false, 10, 3000); 4]);
        let anchor = 16 + index.len() as u64;
        file.extend(&index);
        file.resize(anchor as usize + 40, 0);
        let reader = MemoryReader::with(URL, file);
        // The range covers `styp` and the start of the `sidx` header only.
        let read = read(&reader, &location(0, 30)).await;
        assert_eq!(read.subsegments.len(), 4);
        assert_eq!(read.subsegments[0].range.first, anchor);
        assert_eq!(
            reader.reads()[1],
            (URL.to_owned(), 31, anchor - 1),
            "only the missing bytes are read"
        );
    }

    #[tokio::test]
    async fn hierarchical_and_daisy_chained_indexes_are_followed_in_order() {
        // Top: [index A, index B]. A: [media 100, media 100]. B: [media 50,
        // index C]. C: [media 70].
        let a = sidx(1000, 0, 0, &[(false, 100, 10), (false, 100, 11)]);
        let c = sidx(1000, 0, 0, &[(false, 70, 13)]);
        let c_span = c.len() as u32 + 70;
        let b = sidx(1000, 0, 0, &[(false, 50, 12), (true, c_span, 13)]);
        let a_span = a.len() as u32 + 200;
        let b_span = b.len() as u32 + 50 + c_span;
        let top = sidx(1000, 7, 0, &[(true, a_span, 21), (true, b_span, 25)]);

        let mut file = top.clone();
        let a_at = file.len() as u64;
        file.extend(&a);
        file.resize(file.len() + 200, 1);
        let b_at = file.len() as u64;
        file.extend(&b);
        file.resize(file.len() + 50, 2);
        let c_at = file.len() as u64;
        file.extend(&c);
        file.resize(file.len() + 70, 3);

        let reader = MemoryReader::with(URL, file);
        let read = read(&reader, &location(0, top.len() as u64 - 1)).await;
        let a_media = a_at + a.len() as u64;
        let b_media = b_at + b.len() as u64;
        let c_media = c_at + c.len() as u64;
        assert_eq!(
            ranges(&read),
            [
                (a_media, a_media + 99, 10),
                (a_media + 100, a_media + 199, 11),
                (b_media, b_media + 49, 12),
                (c_media, c_media + 69, 13),
            ]
        );
        assert_eq!(read.earliest_presentation_time, 7);
    }

    #[tokio::test]
    async fn malformed_indexes_are_invalid_content() {
        let top_with = |references: &[Reference], tail: Vec<u8>| {
            let mut file = sidx(1000, 0, 0, references);
            let len = file.len() as u64;
            file.extend(tail);
            (MemoryReader::with(URL, file), location(0, len - 1))
        };
        let cases = [
            (
                top_with(&[(false, 0, 10)], vec![]),
                "a reference has no bytes",
            ),
            (
                top_with(&[(true, 40, 10)], filler(b"moof", 40)),
                "does not point to one",
            ),
            (
                top_with(
                    &[(true, sidx_len(1) + 10, 10)],
                    [sidx(48_000, 0, 0, &[(false, 10, 10)]), vec![0; 10]].concat(),
                ),
                "another stream or timescale",
            ),
            (
                top_with(&[(true, 20, 10)], sidx(1000, 0, 0, &[(false, 1, 1)])),
                "larger than the reference",
            ),
        ];
        for ((reader, location), expected) in cases {
            let message = error(&reader, &location).await;
            assert!(message.contains(expected), "{message}");
            assert!(message.starts_with("invalid content"), "{message}");
        }

        let reader = MemoryReader::with(URL, filler(b"moov", 64));
        assert!(
            error(&reader, &location(0, 63))
                .await
                .contains("holds no Segment Index box")
        );
    }

    #[tokio::test]
    async fn long_index_chains_are_bounded() {
        // Every index holds one subsegment and points to the next.
        let count = MAX_SEGMENT_INDEX_BOXES + 1;
        let link = sidx_len(2) as usize;
        let mut file = Vec::new();
        for position in 0..count {
            let rest = ((count - position - 1) * (link + 1) + 1) as u32;
            file.extend(sidx(1000, 0, 0, &[(false, 1, 10), (true, rest - 1, 10)]));
            file.push(0);
        }
        let reader = MemoryReader::with(URL, file);
        let message = error(&reader, &location(0, link as u64 - 1)).await;
        assert!(message.contains("Segment Index boxes"), "{message}");
    }

    fn selection(video: VideoSelection) -> DashSelectionConfig {
        let mut config = DashConfig::default().selection;
        config.video = video;
        config
    }

    #[test]
    fn mpd_wide_limits_add_up_across_representations() {
        let mpd = MpdBudget::default();
        let spent = |mpd| Spent {
            mpd,
            boxes: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        };
        let per_representation = MAX_SEGMENT_INDEX_BYTES as u64;
        let representations = MAX_MPD_SEGMENT_INDEX_BYTES / MAX_SEGMENT_INDEX_BYTES;
        for _ in 0..representations {
            spent(&mpd).charge_bytes(per_representation).unwrap();
        }
        let error = spent(&mpd).charge_bytes(1).unwrap_err().to_string();
        assert!(error.contains("per MPD"), "{error}");
    }

    #[tokio::test]
    async fn nested_indexes_grouped_before_their_media_are_followed() {
        // Top: [index A, index B], then A and B, then A's media, then B's.
        // A reference runs to the next item of its index, so A's reference
        // spans A alone; each nested index finds its media from its anchor.
        let b_len = u64::from(sidx_len(1));
        let a = sidx(1000, 0, b_len, &[(false, 30, 5), (false, 40, 6)]);
        let b = sidx(1000, 0, 70, &[(false, 20, 7)]);
        let top = sidx(
            1000,
            0,
            0,
            &[(true, a.len() as u32, 11), (true, (b.len() + 90) as u32, 7)],
        );
        let top_len = top.len() as u64;
        let mut file = [top, a, b].concat();
        let media = file.len() as u64;
        file.resize(file.len() + 90, 9);
        let reader = MemoryReader::with(URL, file);
        let read = read(&reader, &location(0, top_len - 1)).await;
        assert_eq!(
            ranges(&read),
            [
                (media, media + 29, 5),
                (media + 30, media + 69, 6),
                (media + 70, media + 89, 7),
            ]
        );
    }

    #[tokio::test]
    async fn the_initialization_ends_where_the_index_box_starts() {
        // `[ftyp][moov][sidx][media]`, with `@indexRange` starting at the
        // file start or at `moov`: neither marks where the initialization
        // data ends.
        let mut file = filler(b"ftyp", 24);
        file.extend(filler(b"moov", 100));
        file.extend(sidx(1000, 0, 0, &[(false, 8, 2000)]));
        let index_end = file.len() - 1;
        file.extend([0; 8]);
        for first in [0, 24] {
            let mpd = format!(
                r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period><AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="{first}-{index_end}"/></Representation></AdaptationSet></Period></MPD>"#
            );
            let reader = MemoryReader::with("https://cdn.example.com/v.mp4", file.clone());
            let document = Url::parse("https://cdn.example.com/m.mpd").unwrap();
            let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
            read_selected(
                &mut manifest,
                &selection(VideoSelection::Highest),
                &[MediaTrack::Video],
                &reader,
                OnIndexFailure::Fail,
            )
            .await
            .unwrap();
            let representation = &manifest.periods[0].adaptation_sets[0].representations[0];
            assert_eq!(
                representation
                    .initialization
                    .as_ref()
                    .and_then(|init| init.byte_range),
                Some(ByteRange {
                    first: 0,
                    last: 123
                }),
                "@indexRange starting at {first}"
            );
        }
    }

    #[tokio::test]
    async fn a_failed_index_fails_a_start_but_not_a_running_recording() {
        // `audio.mp4` is gone; the video index reads fine.
        let mpd = r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="0-51"/></Representation></AdaptationSet>
<AdaptationSet mimeType="audio/mp4"><Representation id="a" bandwidth="1"><BaseURL>audio.mp4</BaseURL><SegmentBase indexRange="0-51"/></Representation></AdaptationSet>
</Period></MPD>"#;
        let mut file = sidx(1000, 0, 0, &[(false, 8, 2000)]);
        file.extend([0; 8]);
        let reader = MemoryReader::with("https://cdn.example.com/v.mp4", file);
        let document = Url::parse("https://cdn.example.com/m.mpd").unwrap();
        let tracks = [MediaTrack::Video, MediaTrack::Audio];
        let config = selection(VideoSelection::Highest);

        let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
        let error = read_selected(
            &mut manifest,
            &config,
            &tracks,
            &reader,
            OnIndexFailure::Fail,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DownloadError::HttpStatus { .. }), "{error}");

        let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
        read_selected(
            &mut manifest,
            &config,
            &tracks,
            &reader,
            OnIndexFailure::Skip,
        )
        .await
        .unwrap();
        let [video, audio] = [0, 1].map(|set| {
            manifest.periods[0].adaptation_sets[set].representations[0]
                .pending_segment_index()
                .is_some()
        });
        assert_eq!((video, audio), (false, true));
        // Needing the audio later fails with what the read failed with.
        let audio = &manifest.periods[0].adaptation_sets[1].representations[0];
        let error = DownloadError::from(audio.segments(chrono::Utc::now()).unwrap_err());
        assert!(
            matches!(error, DownloadError::HttpStatus { status, .. } if status == 410),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_malformed_index_skipped_on_refresh_keeps_its_type() {
        let mpd = r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="0-63"/></Representation></AdaptationSet>
</Period></MPD>"#;
        let reader = MemoryReader::with(URL, filler(b"moov", 64));
        let document = Url::parse("https://cdn.example.com/m.mpd").unwrap();
        let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
        read_selected(
            &mut manifest,
            &selection(VideoSelection::Highest),
            &[MediaTrack::Video],
            &reader,
            OnIndexFailure::Skip,
        )
        .await
        .unwrap();
        let video = &manifest.periods[0].adaptation_sets[0].representations[0];
        let error = DownloadError::from(video.unread_segment_index_error().unwrap());
        assert!(
            matches!(&error, DownloadError::InvalidContent { reason, .. } if reason.contains("holds no Segment Index box")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_resource_indexed_from_several_periods_is_read_once() {
        let period = |start: &str| {
            format!(
                r#"<Period start="{start}"><AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="0-51"/></Representation></AdaptationSet></Period>"#
            )
        };
        let mpd = format!(
            r#"<MPD type="static" mediaPresentationDuration="PT4S">{}{}</MPD>"#,
            period("PT0S"),
            period("PT2S")
        );
        let mut file = sidx(1000, 0, 0, &[(false, 8, 2000)]);
        file.extend([0; 8]);
        let reader = MemoryReader::with("https://cdn.example.com/v.mp4", file);
        let document = Url::parse("https://cdn.example.com/m.mpd").unwrap();
        let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
        read_selected(
            &mut manifest,
            &selection(VideoSelection::Highest),
            &[MediaTrack::Video],
            &reader,
            OnIndexFailure::Fail,
        )
        .await
        .unwrap();
        assert_eq!(reader.reads().len(), 1);
        for period in &manifest.periods {
            let representation = &period.adaptation_sets[0].representations[0];
            assert!(representation.pending_segment_index().is_none());
        }
    }

    #[tokio::test]
    async fn only_selected_representations_are_read() {
        let mpd = r#"<MPD type="static" mediaPresentationDuration="PT4S"><Period>
<AdaptationSet mimeType="video/mp4">
  <Representation id="low" bandwidth="100"><BaseURL>low.mp4</BaseURL><SegmentBase indexRange="0-51"/></Representation>
  <Representation id="high" bandwidth="900"><BaseURL>high.mp4</BaseURL><SegmentBase indexRange="0-51"/></Representation>
</AdaptationSet>
<AdaptationSet mimeType="audio/mp4">
  <Representation id="a" bandwidth="64"><BaseURL>a.mp4</BaseURL><SegmentBase indexRange="0-51"/></Representation>
</AdaptationSet>
</Period></MPD>"#;
        let mut reader = MemoryReader::default();
        for name in ["low", "high", "a"] {
            let mut file = sidx(1000, 0, 0, &[(false, 8, 2000)]);
            file.extend([0; 8]);
            reader
                .files
                .insert(format!("https://cdn.example.com/{name}.mp4"), file);
        }
        let document = Url::parse("https://cdn.example.com/m.mpd").unwrap();

        let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
        read_selected(
            &mut manifest,
            &selection(VideoSelection::Lowest),
            &[MediaTrack::Video],
            &reader,
            OnIndexFailure::Fail,
        )
        .await
        .unwrap();
        let read: Vec<_> = reader.reads().into_iter().map(|read| read.0).collect();
        assert_eq!(read, ["https://cdn.example.com/low.mp4"]);
        let pending = |manifest: &Manifest| -> Vec<String> {
            manifest.periods[0]
                .adaptation_sets
                .iter()
                .flat_map(|set| &set.representations)
                .filter(|representation| representation.pending_segment_index().is_some())
                .map(|representation| representation.id.clone())
                .collect()
        };
        assert_eq!(pending(&manifest), ["high", "a"]);
        let low = &manifest.periods[0].adaptation_sets[0].representations[0];
        let segment = low
            .segments(chrono::Utc::now())
            .unwrap()
            .first()
            .unwrap()
            .unwrap();
        assert_eq!(
            segment.byte_range,
            Some(ByteRange {
                first: 52,
                last: 59
            })
        );

        let mut manifest = Manifest::parse(mpd.as_bytes(), &document).unwrap();
        read_selected(
            &mut manifest,
            &selection(VideoSelection::Highest),
            &[MediaTrack::Video, MediaTrack::Audio],
            &reader,
            OnIndexFailure::Fail,
        )
        .await
        .unwrap();
        assert_eq!(pending(&manifest), ["low"]);
    }
}
