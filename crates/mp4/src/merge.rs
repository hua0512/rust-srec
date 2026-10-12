//! Merging separately packaged fMP4 tracks into one fragmented MP4 stream.
//!
//! MPEG-DASH and CMAF deliver each track (for example video and audio) as its
//! own initialization segment plus media segments. Recording them as a single
//! playable file needs one `moov` describing every track and media fragments
//! whose track IDs, sequence numbers and data offsets agree with it:
//!
//! 1. [`merge_init`] combines single-track initialization segments into one
//!    multi-track [`MergedInit`]. Source `trak` boxes are copied byte for
//!    byte; only the fields that must change are patched.
//! 2. [`MergedInit::parse_media_segment`] validates a source media segment and
//!    returns its fragments, re-addressed to the merged track and moved onto
//!    the caller's timeline.
//! 3. [`FragmentWriter`] serializes fragments in the caller's output order,
//!    assigning strictly increasing `mfhd` sequence numbers (ISO/IEC 14496-12
//!    §8.8.5) and recomputing every `trun` data offset.
//!
//! Media data is never decoded. Inputs whose rewrite could change what a
//! player presents (encryption, explicit base data offsets, auxiliary
//! information offsets, track references, undefined box versions or flags)
//! are rejected with a [`MergeError`] rather than passed through.

mod error;
mod fragment;
mod init;

pub use error::MergeError;
pub use fragment::{FragmentWriter, SourceFragment};
pub use init::{MergedInit, MergedTrack, merge_init};

#[cfg(test)]
mod tests;
