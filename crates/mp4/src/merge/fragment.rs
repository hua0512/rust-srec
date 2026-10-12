//! Re-addressing source media fragments to a [`MergedInit`].

use bytes::Bytes;

use super::{MergeError, MergedInit};
use crate::box_utils::{BoxView, top_level_boxes, write_box_header};
use crate::moof::Moof;

/// One source movie fragment, validated and re-addressed to a merged track.
/// Serialize it with [`FragmentWriter::write`].
#[derive(Debug, Clone)]
pub struct SourceFragment {
    track_index: usize,
    moof: Moof,
    media_data: Bytes,
    /// Start of each `trun`'s samples relative to the start of `media_data`.
    run_offsets: Vec<u64>,
    decode_time: u64,
    duration: u64,
    sample_count: u64,
}

impl SourceFragment {
    /// Index of the merged track (and of the source) this fragment belongs to.
    pub fn track_index(&self) -> usize {
        self.track_index
    }

    /// Decode time of the first sample, in the track timescale, after the
    /// caller's time shift.
    pub fn decode_time(&self) -> u64 {
        self.decode_time
    }

    /// Sum of the sample durations, in the track timescale.
    pub fn duration(&self) -> u64 {
        self.duration
    }

    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// Bytes of sample data carried by this fragment.
    pub fn media_data_len(&self) -> usize {
        self.media_data.len()
    }

    /// Moves the fragment by `delta` ticks of the track timescale, on top of
    /// the time shift it was parsed with. Lets a caller parse a segment
    /// unshifted, choose fragments by time, and shift only those it writes;
    /// a result outside `0..=u64::MAX` is an error and leaves the fragment
    /// unchanged.
    pub fn shift_decode_time(&mut self, delta: i64) -> Result<(), MergeError> {
        let decode_time = u64::try_from(i128::from(self.decode_time) + i128::from(delta))
            .map_err(|_| MergeError::Overflow("decode time after time shift"))?;
        let [traf] = self.moof.trafs.as_mut_slice() else {
            return Err(MergeError::malformed(
                b"moof",
                "fragment lost its track fragment",
            ));
        };
        let tfdt = traf
            .tfdt_mut()
            .ok_or_else(|| MergeError::missing(b"tfdt", b"traf"))?;
        tfdt.set_base_media_decode_time(decode_time);
        self.decode_time = decode_time;
        Ok(())
    }
}

impl MergedInit {
    /// Split a media segment of source `track_index` into fragments
    /// addressed to the merged track.
    ///
    /// `time_shift` (in track timescale units) is added to every decode time,
    /// so that tracks packaged with different presentation time offsets share
    /// one timeline; a result outside `0..=u64::MAX` is an error.
    ///
    /// Segment-level boxes other than `moof`/`mdat` (`styp`, `sidx`, `ssix`,
    /// `prft`, `emsg`, padding) are dropped: they index or annotate the
    /// source segment and nothing in the output refers to them.
    pub fn parse_media_segment(
        &self,
        track_index: usize,
        segment: &Bytes,
        time_shift: i64,
    ) -> Result<Vec<SourceFragment>, MergeError> {
        if track_index >= self.tracks().len() {
            return Err(MergeError::UnknownSource {
                index: track_index,
                sources: self.tracks().len(),
            });
        }

        let mut fragments = Vec::new();
        let mut pending: Option<(BoxView, Moof)> = None;
        for item in top_level_boxes(segment) {
            let item = item?;
            match &item.fourcc {
                b"moof" => {
                    if pending.is_some() {
                        return Err(MergeError::malformed(
                            b"moof",
                            "movie fragment has no media data",
                        ));
                    }
                    let moof = Moof::decode(&segment.slice(item.start..item.end))?;
                    pending = Some((item, moof));
                }
                b"mdat" => {
                    let Some((moof_box, moof)) = pending.take() else {
                        return Err(MergeError::unsupported(
                            b"mdat",
                            "media data does not follow a movie fragment",
                        ));
                    };
                    fragments.push(self.readdress(
                        track_index,
                        segment,
                        &moof_box,
                        moof,
                        &item,
                        time_shift,
                    )?);
                }
                b"styp" | b"sidx" | b"ssix" | b"prft" | b"emsg" | b"free" | b"skip" | b"uuid"
                | b"udta" | b"meta" | b"mfra" => {}
                other => {
                    return Err(MergeError::unsupported(
                        other,
                        "box is not supported in a media segment",
                    ));
                }
            }
        }
        if pending.is_some() {
            return Err(MergeError::malformed(
                b"moof",
                "movie fragment has no media data",
            ));
        }
        Ok(fragments)
    }

    fn readdress(
        &self,
        track_index: usize,
        segment: &Bytes,
        moof_box: &BoxView,
        mut moof: Moof,
        mdat: &BoxView,
        time_shift: i64,
    ) -> Result<SourceFragment, MergeError> {
        let track = &self.tracks()[track_index];
        // CMAF (ISO/IEC 23000-19) fragments carry exactly one track fragment,
        // and a single-track source has no second track to put in another.
        let [traf] = moof.trafs.as_mut_slice() else {
            return Err(MergeError::unsupported(
                b"moof",
                "exactly one track fragment per movie fragment is supported",
            ));
        };
        if traf.tfhd.track_id != track.source_track_id() {
            return Err(MergeError::UnknownTrack {
                track_id: traf.tfhd.track_id,
            });
        }
        // An explicit base offset is "relative to the file" (§8.8.7.1),
        // which is ambiguous for a segment cut out of a stream, and CMAF
        // forbids it. Guessing the origin could point at the wrong bytes.
        if traf.tfhd.base_data_offset.is_some() {
            return Err(MergeError::unsupported(
                b"tfhd",
                "explicit base-data-offset is not supported",
            ));
        }

        // With one track fragment and no explicit base, both
        // default-base-is-moof and the legacy rule put the base at the first
        // byte of the moof (§8.8.7.1). A run without data_offset continues
        // where the previous run ended, or starts at the base (§8.8.8.1).
        let base = moof_box.start as u64;
        let default_duration = traf
            .tfhd
            .default_sample_duration
            .unwrap_or(track.default_sample_duration);
        let default_size = traf
            .tfhd
            .default_sample_size
            .unwrap_or(track.default_sample_size);
        let media_start = mdat.body_start as u64;
        let media_end = mdat.end as u64;
        let mut cursor = base;
        let mut run_offsets = Vec::new();
        let mut duration = 0u64;
        let mut sample_count = 0u64;
        for trun in traf.truns() {
            let start = match trun.data_offset {
                Some(offset) => base.checked_add_signed(i64::from(offset)),
                None => Some(cursor),
            };
            let totals = trun.totals(default_duration, default_size);
            let end = start.and_then(|start| start.checked_add(totals.size));
            let (Some(start), Some(end)) = (start, end) else {
                return Err(MergeError::malformed(
                    b"trun",
                    "sample data offset overflows",
                ));
            };
            if start < media_start || end > media_end {
                return Err(MergeError::malformed(
                    b"trun",
                    "sample data lies outside the following mdat",
                ));
            }
            run_offsets.push(start - media_start);
            cursor = end;
            duration = duration
                .checked_add(totals.duration)
                .ok_or(MergeError::Overflow("fragment duration"))?;
            sample_count += u64::from(trun.sample_count());
        }

        // §8.8.7.1: duration-is-empty means the fragment covers its duration
        // with no samples.
        if traf.tfhd.duration_is_empty && sample_count != 0 {
            return Err(MergeError::malformed(
                b"tfhd",
                "duration-is-empty is set but the fragment has samples",
            ));
        }
        // The sample entry the samples use must exist in the merged `stsd`.
        if let Some(index) = traf.tfhd.sample_description_index
            && !(1..=track.sample_entry_count).contains(&index)
        {
            return Err(MergeError::malformed(
                b"tfhd",
                "sample description index refers to no sample entry",
            ));
        }

        let tfdt = traf
            .tfdt_mut()
            .ok_or_else(|| MergeError::missing(b"tfdt", b"traf"))?;
        let decode_time =
            u64::try_from(i128::from(tfdt.base_media_decode_time()) + i128::from(time_shift))
                .map_err(|_| MergeError::Overflow("decode time after time shift"))?;
        tfdt.set_base_media_decode_time(decode_time);

        traf.tfhd.track_id = track.track_id();
        traf.tfhd.default_base_is_moof = true;
        // The writer patches the real offsets; every run must carry one
        // because runs no longer start where the source put them.
        for trun in traf.truns_mut() {
            trun.data_offset = Some(0);
        }

        Ok(SourceFragment {
            track_index,
            moof,
            media_data: segment.slice(mdat.body_start..mdat.end),
            run_offsets,
            decode_time,
            duration,
            sample_count,
        })
    }
}

/// Serializes fragments in output order.
///
/// Sequence numbers start at 1 and increase by one per fragment across all
/// tracks, as `mfhd` requires them to increase in file order (§8.8.5).
#[derive(Debug)]
pub struct FragmentWriter {
    next_sequence: Option<u32>,
}

impl Default for FragmentWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl FragmentWriter {
    pub fn new() -> Self {
        Self {
            next_sequence: Some(1),
        }
    }

    /// Append `fragment` as a `moof` + `mdat` pair to `out`. On error `out`
    /// is left as it was.
    pub fn write(
        &mut self,
        mut fragment: SourceFragment,
        out: &mut Vec<u8>,
    ) -> Result<(), MergeError> {
        let sequence = self.next_sequence.ok_or(MergeError::SequenceExhausted)?;
        let start = out.len();
        let result = Self::write_pair(&mut fragment, sequence, out);
        if result.is_err() {
            out.truncate(start);
        } else {
            self.next_sequence = sequence.checked_add(1);
        }
        result
    }

    fn write_pair(
        fragment: &mut SourceFragment,
        sequence: u32,
        out: &mut Vec<u8>,
    ) -> Result<(), MergeError> {
        fragment.moof.sequence_number = sequence;
        let moof_start = out.len();
        let positions = fragment.moof.encode(out)?;
        let moof_len = (out.len() - moof_start) as u64;

        let media_len = fragment.media_data.len() as u64;
        let header_start = out.len();
        write_box_header(out, b"mdat", media_len);
        let header_len = (out.len() - header_start) as u64;

        // `readdress` gave every run a data offset and recorded one source
        // offset per run; anything else would write samples at wrong offsets.
        if positions.len() != fragment.run_offsets.len() {
            return Err(MergeError::malformed(
                b"trun",
                "run count changed during rewrite",
            ));
        }
        // `default-base-is-moof` is set, so offsets count from the moof start.
        for (position, run_offset) in positions.into_iter().zip(&fragment.run_offsets) {
            let Some(position) = position else {
                return Err(MergeError::malformed(b"trun", "run lost its data offset"));
            };
            let offset = i32::try_from(moof_len + header_len + run_offset)
                .map_err(|_| MergeError::Overflow("trun data offset"))?;
            out[position..position + 4].copy_from_slice(&offset.to_be_bytes());
        }
        out.extend_from_slice(&fragment.media_data);
        Ok(())
    }
}
